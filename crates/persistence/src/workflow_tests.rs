use std::{
    collections::VecDeque,
    future::{Future, ready},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use einfach_mail_application::{
    AccountCacheFactory, AccountSessionFactory, AccountSessionWorkflowFactory, CalendarCacheError,
    CalendarDeltaPageResponse, CalendarDeltaRequest, CalendarOperationError, CalendarProvider,
    CalendarWorkflowError, MailCache, MailCacheError, MailOperationError, MailProvider,
    MailSendOutcome, MailSendProviderError, MailSendWorkflowError, MailSyncProgress, MailWorkflow,
    MailWorkflowError, MessageBodyForDisplay, MessageDeltaPageResponse, MessageDeltaRequest,
    MessageDeltaResponse,
};
use einfach_mail_domain::{
    AccountId, CalendarViewRange, CalendarViewReference, FolderId, FolderReference, MailDraft,
    MailFolder, MailMessage, MailRecipient, MailSubject, MessageBody, MessageBodyType,
    MessageDeltaChange, MessageDeltaLink, MessageDeltaPage, MessageDeltaProgress, MessageId,
    MessageMetadataPatch, ReadState,
};
use tempfile::TempDir;
use time::{Date, Month};

use crate::{security, store::SqliteMailCache};

const DATABASE_KEY_BYTE: u8 = 0x62;

type PageQueue = VecDeque<Result<MessageDeltaResponse, MailOperationError>>;
type ReadStateGate = Arc<Mutex<Option<(smol::channel::Sender<()>, smol::channel::Receiver<()>)>>>;

#[derive(Clone)]
struct FixtureProvider {
    account_id: AccountId,
    folders: Vec<MailFolder>,
    folder_snapshots: Arc<Mutex<VecDeque<Vec<MailFolder>>>>,
    folder_list_calls: Arc<AtomicUsize>,
    pages: Arc<Mutex<PageQueue>>,
    requests: Arc<Mutex<Vec<MessageDeltaRequest>>>,
    body_result: Result<MessageBody, MailOperationError>,
    body_fetches: Arc<AtomicUsize>,
    read_state_updates: Arc<Mutex<Vec<(MessageId, ReadState)>>>,
    read_state_gate: ReadStateGate,
    send_calls: Arc<AtomicUsize>,
    send_result: Result<(), MailSendProviderError>,
    body_gate: Option<BodyFetchGate>,
    delta_gate: Option<DeltaPageGate>,
    calendar_gate: Option<CalendarSyncGate>,
}

struct FixtureCacheFactory {
    database_directory: std::path::PathBuf,
}

impl AccountCacheFactory for FixtureCacheFactory {
    type Cache = SqliteMailCache;

    fn open_for_account(
        &self,
        account_id: AccountId,
    ) -> impl Future<Output = Result<Self::Cache, MailCacheError>> + Send + '_ {
        let database_path = self.database_directory.join("mail.sqlite3");
        async move {
            Ok(
                SqliteMailCache::open_with_test_key(database_path, account_id, DATABASE_KEY_BYTE)
                    .await,
            )
        }
    }

    fn remove_for_account<'a>(
        &'a self,
        _: &'a AccountId,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + 'a {
        ready(Err(MailCacheError::Unavailable))
    }
}

#[derive(Clone)]
struct BodyFetchGate {
    started: smol::channel::Sender<()>,
    release: smol::channel::Receiver<()>,
}

#[derive(Clone)]
struct DeltaPageGate {
    entered: Arc<AtomicUsize>,
    started: smol::channel::Sender<()>,
    release: smol::channel::Receiver<()>,
}

#[derive(Clone)]
struct CalendarSyncGate {
    started: smol::channel::Sender<()>,
    release: smol::channel::Receiver<()>,
}

impl FixtureProvider {
    fn new(
        account_id: AccountId,
        folders: Vec<MailFolder>,
        pages: impl IntoIterator<Item = Result<MessageDeltaResponse, MailOperationError>>,
        body_result: Result<MessageBody, MailOperationError>,
    ) -> Self {
        Self {
            account_id,
            folders,
            folder_snapshots: Arc::new(Mutex::new(VecDeque::new())),
            folder_list_calls: Arc::new(AtomicUsize::new(0)),
            pages: Arc::new(Mutex::new(pages.into_iter().collect())),
            requests: Arc::new(Mutex::new(Vec::new())),
            body_result,
            body_fetches: Arc::new(AtomicUsize::new(0)),
            read_state_updates: Arc::new(Mutex::new(Vec::new())),
            read_state_gate: Arc::new(Mutex::new(None)),
            send_calls: Arc::new(AtomicUsize::new(0)),
            send_result: Err(MailSendProviderError::ServiceUnavailable),
            body_gate: None,
            delta_gate: None,
            calendar_gate: None,
        }
    }

    fn with_send_result(mut self, result: Result<(), MailSendProviderError>) -> Self {
        self.send_result = result;
        self
    }

    fn hold_next_read_state(&self) -> (smol::channel::Receiver<()>, smol::channel::Sender<()>) {
        let (started, started_receiver) = smol::channel::bounded(1);
        let (release, release_receiver) = smol::channel::bounded(1);
        *self
            .read_state_gate
            .lock()
            .expect("fixture read-state gate mutex is healthy") = Some((started, release_receiver));
        (started_receiver, release)
    }
}

impl MailProvider for FixtureProvider {
    fn list_folders(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<Vec<MailFolder>, MailOperationError>> + Send + '_ {
        let folders = self.folders.clone();
        let snapshots = Arc::clone(&self.folder_snapshots);
        let calls = Arc::clone(&self.folder_list_calls);
        let matches_account = account_id == &self.account_id;
        async move {
            if matches_account {
                calls.fetch_add(1, Ordering::Relaxed);
                Ok(snapshots
                    .lock()
                    .expect("synthetic folder snapshot mutex is not poisoned")
                    .pop_front()
                    .unwrap_or(folders))
            } else {
                Err(MailOperationError::AccountMismatch)
            }
        }
    }

    fn list_messages(
        &self,
        _folder: &FolderReference,
    ) -> impl Future<Output = Result<Vec<MailMessage>, MailOperationError>> + Send + '_ {
        ready(Ok(Vec::new()))
    }

    fn message_delta_page(
        &self,
        folder: &FolderReference,
        request: MessageDeltaRequest,
    ) -> impl Future<Output = Result<MessageDeltaResponse, MailOperationError>> + Send + '_ {
        let account_id = folder.account_id().clone();
        let expected_account = self.account_id.clone();
        let pages = Arc::clone(&self.pages);
        let requests = Arc::clone(&self.requests);
        let delta_gate = self.delta_gate.clone();
        async move {
            if account_id != expected_account {
                return Err(MailOperationError::AccountMismatch);
            }
            requests
                .lock()
                .expect("synthetic request log mutex is not poisoned")
                .push(request);
            if let Some(gate) = delta_gate
                && gate.entered.fetch_add(1, Ordering::Relaxed) == 0
            {
                gate.started
                    .send(())
                    .await
                    .map_err(|_| MailOperationError::ServiceUnavailable)?;
                gate.release
                    .recv()
                    .await
                    .map_err(|_| MailOperationError::ServiceUnavailable)?;
            }
            pages
                .lock()
                .expect("synthetic delta page mutex is not poisoned")
                .pop_front()
                .unwrap_or(Err(MailOperationError::ServiceUnavailable))
        }
    }

    fn fetch_message_body(
        &self,
        folder: &FolderReference,
        _message_id: &MessageId,
    ) -> impl Future<Output = Result<MessageBody, MailOperationError>> + Send + '_ {
        let account_id = folder.account_id().clone();
        let expected_account = self.account_id.clone();
        let result = self.body_result.clone();
        let fetches = Arc::clone(&self.body_fetches);
        let body_gate = self.body_gate.clone();
        async move {
            if account_id != expected_account {
                return Err(MailOperationError::AccountMismatch);
            }
            fetches.fetch_add(1, Ordering::Relaxed);
            if let Some(gate) = body_gate {
                gate.started
                    .send(())
                    .await
                    .map_err(|_| MailOperationError::ServiceUnavailable)?;
                gate.release
                    .recv()
                    .await
                    .map_err(|_| MailOperationError::ServiceUnavailable)?;
            }
            result
        }
    }

    fn set_message_read_state(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
        requested: ReadState,
    ) -> impl Future<Output = Result<ReadState, MailOperationError>> + Send + '_ {
        let matches_account = folder.account_id() == &self.account_id;
        let read_state_gate = if matches_account {
            self.read_state_gate
                .lock()
                .expect("fixture read-state gate mutex is healthy")
                .take()
        } else {
            None
        };
        let updates = Arc::clone(&self.read_state_updates);
        let message_id = message_id.clone();
        async move {
            if !matches_account {
                return Err(MailOperationError::AccountMismatch);
            }
            if let Some((started, release)) = read_state_gate {
                started
                    .send(())
                    .await
                    .map_err(|_| MailOperationError::ServiceUnavailable)?;
                release
                    .recv()
                    .await
                    .map_err(|_| MailOperationError::ServiceUnavailable)?;
            }
            updates
                .lock()
                .expect("synthetic read-state log mutex is healthy")
                .push((message_id, requested));
            Ok(requested)
        }
    }

    fn send_mail(
        &self,
        account_id: &AccountId,
        _draft: MailDraft,
    ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_ {
        self.send_calls.fetch_add(1, Ordering::Relaxed);
        if account_id != &self.account_id {
            return ready(Err(MailSendProviderError::AccountMismatch));
        }
        ready(self.send_result)
    }
}

impl CalendarProvider for FixtureProvider {
    fn calendar_delta_page(
        &self,
        view: &CalendarViewReference,
        _request: CalendarDeltaRequest,
    ) -> impl Future<Output = Result<CalendarDeltaPageResponse, CalendarOperationError>> + Send + '_
    {
        let matches_account = view.account_id() == &self.account_id;
        let gate = self.calendar_gate.clone();
        async move {
            if !matches_account {
                return Err(CalendarOperationError::AccountMismatch);
            }
            if let Some(gate) = gate {
                gate.started
                    .send(())
                    .await
                    .map_err(|_| CalendarOperationError::ServiceUnavailable)?;
                gate.release
                    .recv()
                    .await
                    .map_err(|_| CalendarOperationError::ServiceUnavailable)?;
            }
            Err(CalendarOperationError::ServiceUnavailable)
        }
    }
}

fn account_id() -> AccountId {
    AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid")
}

fn folder(account_id: &AccountId) -> MailFolder {
    let reference = FolderReference::new(
        account_id.clone(),
        FolderId::try_from("folder-inbox".to_owned()).expect("synthetic folder ID is valid"),
    );
    MailFolder::new(reference, None, "Synthetic Inbox".to_owned())
        .expect("synthetic folder is valid")
}

fn message_id() -> MessageId {
    MessageId::try_from("message-1".to_owned()).expect("synthetic message ID is valid")
}

fn draft(subject: &str, body: &str) -> MailDraft {
    MailDraft::new(
        "ada@example.test"
            .parse::<MailRecipient>()
            .expect("synthetic recipient is valid"),
        subject
            .parse::<MailSubject>()
            .expect("synthetic subject is valid"),
        body.to_owned(),
    )
}

fn calendar_view(account: &AccountId) -> CalendarViewReference {
    let start_date = Date::from_calendar_date(2030, Month::January, 7)
        .expect("synthetic calendar view start is valid");
    let end_date = start_date
        .checked_add(time::Duration::days(7))
        .expect("synthetic calendar view end is valid");
    let range = CalendarViewRange::new(
        start_date.midnight().assume_utc(),
        end_date.midnight().assume_utc(),
        start_date,
        end_date,
    )
    .expect("synthetic calendar view range is valid");
    CalendarViewReference::new(account.clone(), range)
}

fn first_page(cutoff: SystemTime, next_link: MessageDeltaLink) -> MessageDeltaPage {
    let mut recent = MessageMetadataPatch::new();
    recent.set_subject(Some("Synthetic subject".to_owned()));
    recent.set_from(None);
    recent.set_sender(None);
    recent.set_received_at(cutoff);
    recent.set_read_state(ReadState::Unread);
    let mut old = MessageMetadataPatch::new();
    old.set_subject(Some("Outside cutoff".to_owned()));
    old.set_from(None);
    old.set_sender(None);
    old.set_received_at(cutoff - Duration::from_nanos(1));
    old.set_read_state(ReadState::Read);
    MessageDeltaPage::new(
        vec![
            MessageDeltaChange::Upsert {
                id: message_id(),
                patch: recent,
            },
            MessageDeltaChange::Upsert {
                id: MessageId::try_from("old-message".to_owned())
                    .expect("synthetic message ID is valid"),
                patch: old,
            },
        ],
        MessageDeltaProgress::Next(next_link),
    )
}

fn final_page(delta_link: MessageDeltaLink) -> MessageDeltaPage {
    let mut patch = MessageMetadataPatch::new();
    patch.set_read_state(ReadState::Read);
    MessageDeltaPage::new(
        vec![MessageDeltaChange::Upsert {
            id: message_id(),
            patch,
        }],
        MessageDeltaProgress::Complete(delta_link),
    )
}

fn page_response(page: MessageDeltaPage) -> MessageDeltaResponse {
    MessageDeltaResponse::Page(MessageDeltaPageResponse::new(page, 1_024))
}

fn ignore_sync_progress(_: MailSyncProgress) {}

fn complete_patch(subject: String, received_at: SystemTime) -> MessageMetadataPatch {
    let mut patch = MessageMetadataPatch::new();
    patch.set_subject(Some(subject));
    patch.set_from(None);
    patch.set_sender(None);
    patch.set_received_at(received_at);
    patch.set_read_state(ReadState::Unread);
    patch
}

async fn open_cache(directory: &TempDir, account_id: AccountId) -> SqliteMailCache {
    SqliteMailCache::open_with_test_key(
        directory.path().join("mail.sqlite3"),
        account_id,
        DATABASE_KEY_BYTE,
    )
    .await
}

#[test]
fn read_state_write_reserves_folder_until_graph_confirmation_and_cache_commit() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Synthetic message".to_owned(), started_at),
            }],
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-initial"
                    .to_owned(),
            )),
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            [Ok(page_response(page))],
            Err(MailOperationError::ServiceUnavailable),
        );
        let (update_started, release_update) = provider.hold_next_read_state();
        let updates = Arc::clone(&provider.read_state_updates);
        let requests = Arc::clone(&provider.requests);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = Arc::new(MailWorkflow::new(provider, cache.clone()));
        workflow
            .synchronize_account(&account, started_at, &ignore_sync_progress)
            .await
            .expect("synthetic message is cached before its read-state action");

        let executor = smol::Executor::new();
        let update_workflow = Arc::clone(&workflow);
        let update_folder = folder_reference.clone();
        let (update_result, sync_result) = smol::block_on(executor.run(async {
            let update_task = executor.spawn(async move {
                update_workflow
                    .set_message_read_state(&update_folder, &message_id(), ReadState::Read)
                    .await
            });
            update_started
                .recv()
                .await
                .expect("synthetic provider waits before confirming the write");
            let sync_result = workflow
                .synchronize_account(
                    &account,
                    started_at + Duration::from_secs(1),
                    &ignore_sync_progress,
                )
                .await;
            release_update
                .send(())
                .await
                .expect("synthetic provider releases its confirmation");
            (update_task.await, sync_result)
        }));

        assert_eq!(
            update_result.expect("read-state write completes"),
            einfach_mail_application::ReadStateUpdateOutcome::ConfirmedAndCached(ReadState::Read)
        );
        assert_eq!(sync_result, Err(MailWorkflowError::OperationInProgress));
        assert_eq!(
            requests
                .lock()
                .expect("synthetic request log mutex is healthy")
                .len(),
            1
        );
        assert_eq!(
            updates
                .lock()
                .expect("synthetic read-state log mutex is healthy")
                .as_slice(),
            [(message_id(), ReadState::Read)]
        );
        assert_eq!(
            cache
                .message_read_state(&folder_reference, &message_id())
                .await
                .expect("confirmed state can be read from the encrypted cache"),
            Some(ReadState::Read)
        );

        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn interrupted_sync_resumes_its_checkpoint_and_body_reopens_from_encrypted_cache() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let cutoff = started_at - Duration::from_secs(90 * 24 * 60 * 60);
        let next_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/messages?$skiptoken=synthetic-resume".to_owned(),
        );
        let final_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-final".to_owned(),
        );
        let initial_provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder.clone()],
            [
                Ok(page_response(first_page(cutoff, next_link.clone()))),
                Err(MailOperationError::ServiceUnavailable),
            ],
            Ok(MessageBody::new(
                MessageBodyType::Html,
                "<p onclick=\"run()\">Synthetic body</p><script>private marker</script>".to_owned(),
            )),
        );
        let initial_requests = Arc::clone(&initial_provider.requests);
        let initial_body_fetches = Arc::clone(&initial_provider.body_fetches);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(initial_provider, cache.clone());

        assert_eq!(
            workflow
                .synchronize_account(&account, started_at, &ignore_sync_progress)
                .await,
            Err(MailWorkflowError::Provider(
                MailOperationError::ServiceUnavailable
            ))
        );
        let saved_state = workflow
            .cached_sync_state(&folder_reference)
            .await
            .expect("interrupted sync state is readable")
            .expect("initial cutoff was saved before the request");
        assert_eq!(saved_state.cutoff(), cutoff);
        assert_eq!(saved_state.next_link(), Some(&next_link));
        assert!(saved_state.delta_link().is_none());
        assert_eq!(
            workflow
                .list_cached_messages(&folder_reference)
                .await
                .expect("page metadata is cached before interruption")
                .len(),
            1
        );
        assert_eq!(
            initial_requests
                .lock()
                .expect("synthetic request log mutex is not poisoned")
                .as_slice(),
            [
                MessageDeltaRequest::Initial,
                MessageDeltaRequest::Continue(next_link.clone()),
            ]
        );
        assert_eq!(initial_body_fetches.load(Ordering::Relaxed), 0);
        drop(workflow);

        let resume_provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            [Ok(page_response(final_page(final_link.clone())))],
            Ok(MessageBody::new(
                MessageBodyType::Html,
                "<p onclick=\"run()\">Synthetic body</p><script>private marker</script>".to_owned(),
            )),
        );
        let resume_requests = Arc::clone(&resume_provider.requests);
        let body_fetches = Arc::clone(&resume_provider.body_fetches);
        let resumed_workflow = MailWorkflow::new(resume_provider, cache.clone());
        let report = resumed_workflow
            .synchronize_account(
                &account,
                started_at + Duration::from_secs(86_400),
                &ignore_sync_progress,
            )
            .await
            .expect("the interrupted folder round resumes");
        assert_eq!(report.folder_count(), 1);
        assert_eq!(report.page_count(), 1);
        assert_eq!(
            resume_requests
                .lock()
                .expect("synthetic request log mutex is not poisoned")
                .as_slice(),
            [MessageDeltaRequest::Continue(next_link)]
        );
        let complete_state = resumed_workflow
            .cached_sync_state(&folder_reference)
            .await
            .expect("completed cursor is readable")
            .expect("folder cursor remains present");
        assert_eq!(complete_state.cutoff(), cutoff);
        assert_eq!(complete_state.delta_link(), Some(&final_link));
        assert!(complete_state.next_link().is_none());
        assert!(complete_state.last_synchronized_at().is_some());

        let opened = resumed_workflow
            .open_message(&folder_reference, &message_id())
            .await
            .expect("opening a message fetches and caches its body");
        assert!(!opened.loaded_from_cache());
        assert!(matches!(
            opened.body(),
            MessageBodyForDisplay::SanitizedHtml(html)
                if html.as_str() == "<p>Synthetic body</p>"
        ));
        let (_, cached_message) = cache
            .read_message(&folder_reference, &message_id())
            .await
            .expect("raw cached body can be read")
            .expect("message remains cached");
        assert_eq!(
            cached_message.expect("body was persisted").content(),
            "<p onclick=\"run()\">Synthetic body</p><script>private marker</script>"
        );
        assert_eq!(body_fetches.load(Ordering::Relaxed), 1);
        drop(resumed_workflow);
        cache.close_for_tests().await;

        let reopened_cache = open_cache(&directory, account.clone()).await;
        let offline_provider = FixtureProvider::new(
            account,
            vec![folder(&account_id())],
            [],
            Err(MailOperationError::ServiceUnavailable),
        );
        let offline_fetches = Arc::clone(&offline_provider.body_fetches);
        let offline_workflow = MailWorkflow::new(offline_provider, reopened_cache.clone());
        let reopened_message = offline_workflow
            .open_message(&folder_reference, &message_id())
            .await
            .expect("cached body opens without a provider request");
        assert!(reopened_message.loaded_from_cache());
        assert!(matches!(
            reopened_message.body(),
            MessageBodyForDisplay::SanitizedHtml(html)
                if html.as_str() == "<p>Synthetic body</p>"
        ));
        let (_, reopened_cached_body) = reopened_cache
            .read_message(&folder_reference, &message_id())
            .await
            .expect("raw cached body can be read after restart")
            .expect("message remains cached after restart");
        assert_eq!(
            reopened_cached_body
                .expect("raw body remains in the encrypted cache")
                .content(),
            "<p onclick=\"run()\">Synthetic body</p><script>private marker</script>"
        );
        assert_eq!(offline_fetches.load(Ordering::Relaxed), 0);
        drop(offline_workflow);
        reopened_cache.close_for_tests().await;
    });
}

#[test]
fn opening_a_missing_body_reserves_its_folder_against_delta_sync() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let old_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/messages?$deltatoken=before-race".to_owned(),
        );
        let new_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/messages?$deltatoken=after-race".to_owned(),
        );
        let initial_page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Before sync".to_owned(), started_at),
            }],
            MessageDeltaProgress::Complete(old_link),
        );
        let cache = open_cache(&directory, account.clone()).await;
        let initial_workflow = MailWorkflow::new(
            FixtureProvider::new(
                account.clone(),
                vec![cached_folder.clone()],
                [Ok(page_response(initial_page))],
                Err(MailOperationError::ServiceUnavailable),
            ),
            cache.clone(),
        );
        initial_workflow
            .synchronize_account(&account, started_at, &ignore_sync_progress)
            .await
            .expect("initial metadata sync completes");

        let update_page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("After sync".to_owned(), started_at),
            }],
            MessageDeltaProgress::Complete(new_link),
        );
        let mut provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            [Ok(page_response(update_page))],
            Ok(MessageBody::new(
                MessageBodyType::Text,
                "Synthetic fetched body".to_owned(),
            )),
        );
        let (started_sender, started_receiver) = smol::channel::unbounded();
        let (release_sender, release_receiver) = smol::channel::bounded(1);
        provider.body_gate = Some(BodyFetchGate {
            started: started_sender,
            release: release_receiver,
        });
        let workflow = Arc::new(MailWorkflow::new(provider, cache.clone()));
        let executor = smol::Executor::new();
        let open_workflow = Arc::clone(&workflow);
        let open_folder = folder_reference.clone();
        let open_message_id = message_id();

        let (open_result, sync_result) = smol::block_on(executor.run(async {
            let open_task = executor.spawn(async move {
                open_workflow
                    .open_message(&open_folder, &open_message_id)
                    .await
            });
            started_receiver
                .recv()
                .await
                .expect("body fetch reaches the synchronization gate");
            let sync_result = workflow
                .synchronize_account(
                    &account,
                    started_at + Duration::from_secs(1),
                    &ignore_sync_progress,
                )
                .await;
            release_sender
                .send(())
                .await
                .expect("body fetch gate is still open");
            (open_task.await, sync_result)
        }));

        let opened = open_result.expect("message opens after the body fetch is released");
        assert!(!opened.loaded_from_cache());
        assert_eq!(
            sync_result,
            Err(MailWorkflowError::OperationInProgress),
            "a delta page cannot commit while a missing body is being fetched"
        );
        let (_, cached_body) = cache
            .read_message(&folder_reference, &message_id())
            .await
            .expect("message remains readable after the first open")
            .expect("message metadata remains cached");
        assert_eq!(
            cached_body.expect("fetched body is cached").content(),
            "Synthetic fetched body"
        );

        workflow
            .synchronize_account(
                &account,
                started_at + Duration::from_secs(2),
                &ignore_sync_progress,
            )
            .await
            .expect("delta sync completes after the open releases its reservation");
        let (updated_message, cached_body) = cache
            .read_message(&folder_reference, &message_id())
            .await
            .expect("updated message remains readable")
            .expect("updated message remains cached");
        assert_eq!(updated_message.metadata().subject(), Some("After sync"));
        assert!(cached_body.is_none());

        drop(workflow);
        drop(initial_workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn factory_sessions_share_folder_reservations_during_body_fetch() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let cutoff = started_at - Duration::from_secs(90 * 24 * 60 * 60);
        let cache = open_cache(&directory, account.clone()).await;
        cache
            .replace_folders(&account, vec![cached_folder.clone()])
            .await
            .expect("synthetic folder is cached");
        cache
            .ensure_account_sync_scope(&account, cutoff)
            .await
            .expect("account cutoff is saved");
        let initial_state = cache
            .ensure_sync_state(&folder_reference, cutoff)
            .await
            .expect("initial checkpoint is saved");
        cache
            .commit_delta_page(
                &folder_reference,
                &initial_state,
                MessageDeltaPage::new(
                    vec![MessageDeltaChange::Upsert {
                        id: message_id(),
                        patch: complete_patch("Cached message".to_owned(), started_at),
                    }],
                    MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                        "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken=baseline"
                            .to_owned(),
                    )),
                ),
                started_at,
            )
            .await
            .expect("synthetic baseline sync commits");
        cache.close_for_tests().await;

        let (started_sender, started_receiver) = smol::channel::unbounded();
        let (release_sender, release_receiver) = smol::channel::bounded(1);
        let mut provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            [],
            Ok(MessageBody::new(
                MessageBodyType::Text,
                "Synthetic fetched body".to_owned(),
            )),
        );
        provider.body_gate = Some(BodyFetchGate {
            started: started_sender,
            release: release_receiver,
        });
        let requests = Arc::clone(&provider.requests);
        let factory = AccountSessionWorkflowFactory::new(
            provider,
            FixtureCacheFactory {
                database_directory: directory.path().to_path_buf(),
            },
        );
        let opening_session = factory
            .open_session(account.clone())
            .await
            .expect("first mail session opens");
        let syncing_session = factory
            .open_session(account.clone())
            .await
            .expect("second mail session opens");
        let executor = smol::Executor::new();
        let opening_session_task = Arc::clone(&opening_session);
        let opening_folder = folder_reference.clone();
        let opening_message = message_id();

        let (open_result, sync_result) = smol::block_on(executor.run(async {
            let open_task = executor.spawn(async move {
                opening_session_task
                    .mail_session()
                    .open_message(&opening_folder, &opening_message)
                    .await
            });
            started_receiver
                .recv()
                .await
                .expect("body fetch reaches the synchronization gate");
            let sync_result = syncing_session
                .mail_session()
                .synchronize_account(
                    &account,
                    started_at + Duration::from_secs(1),
                    &ignore_sync_progress,
                )
                .await;
            release_sender
                .send(())
                .await
                .expect("body fetch gate is still open");
            (open_task.await, sync_result)
        }));

        assert!(
            !open_result
                .expect("message open completes")
                .loaded_from_cache()
        );
        assert_eq!(sync_result, Err(MailWorkflowError::OperationInProgress));
        assert!(
            requests
                .lock()
                .expect("synthetic request log mutex is not poisoned")
                .is_empty()
        );
        drop(opening_session);
        drop(syncing_session);
    });
}

#[test]
fn calendar_sync_reservation_blocks_sign_out_cache_removal() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let (started_sender, started_receiver) = smol::channel::bounded(1);
        let (release_sender, release_receiver) = smol::channel::bounded(1);
        let mut provider = FixtureProvider::new(
            account.clone(),
            Vec::new(),
            [],
            Err(MailOperationError::ServiceUnavailable),
        );
        provider.calendar_gate = Some(CalendarSyncGate {
            started: started_sender,
            release: release_receiver,
        });
        let factory = AccountSessionWorkflowFactory::new(
            provider,
            FixtureCacheFactory {
                database_directory: directory.path().to_path_buf(),
            },
        );
        let session = factory
            .open_session(account.clone())
            .await
            .expect("account session opens");
        let calendar = session.calendar_session();
        let view = calendar_view(&account);
        let sync_calendar = Arc::clone(&calendar);
        let sync_view = view.clone();
        let sync_session = Arc::clone(&session);
        let executor = smol::Executor::new();

        let (sync_result, session_removal_result, factory_removal_result) = executor
            .run(async {
                let sync_task =
                    executor.spawn(async move { sync_calendar.synchronize_view(&sync_view).await });
                started_receiver
                    .recv()
                    .await
                    .expect("calendar request reaches the synthetic gate");
                let session_removal_result = sync_session.remove_account_cache(&account).await;
                let factory_removal_result = factory.remove_account_cache(&account).await;
                release_sender
                    .send(())
                    .await
                    .expect("calendar sync gate is still open");
                (
                    sync_task.await,
                    session_removal_result,
                    factory_removal_result,
                )
            })
            .await;

        assert_eq!(
            sync_result,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::ServiceUnavailable
            ))
        );
        assert_eq!(
            session_removal_result,
            Err(MailCacheError::OperationInProgress)
        );
        assert_eq!(
            factory_removal_result,
            Err(MailCacheError::OperationInProgress)
        );
        calendar
            .load_cached_view(&view)
            .await
            .expect("the shared cache remains available after sign-out is blocked");
    });
}

#[test]
fn factory_account_reservation_prevents_folder_snapshot_replacement_race() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Account snapshot survives".to_owned(), started_at),
            }],
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken=complete"
                    .to_owned(),
            )),
        );
        let mut provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder.clone()],
            [Ok(page_response(page))],
            Err(MailOperationError::ServiceUnavailable),
        );
        provider
            .folder_snapshots
            .lock()
            .expect("synthetic folder snapshot mutex is not poisoned")
            .extend([vec![cached_folder.clone()], Vec::new()]);
        let (started_sender, started_receiver) = smol::channel::unbounded();
        let (release_sender, release_receiver) = smol::channel::bounded(1);
        provider.delta_gate = Some(DeltaPageGate {
            entered: Arc::new(AtomicUsize::new(0)),
            started: started_sender,
            release: release_receiver,
        });
        let folder_list_calls = Arc::clone(&provider.folder_list_calls);
        let factory = AccountSessionWorkflowFactory::new(
            provider,
            FixtureCacheFactory {
                database_directory: directory.path().to_path_buf(),
            },
        );
        let syncing_session = factory
            .open_session(account.clone())
            .await
            .expect("first mail session opens");
        let competing_session = factory
            .open_session(account.clone())
            .await
            .expect("second mail session opens");
        let executor = smol::Executor::new();
        let first_account = account.clone();

        let (sync_result, competing_result) = smol::block_on(executor.run(async {
            let sync_task = executor.spawn(async move {
                syncing_session
                    .mail_session()
                    .synchronize_account(&first_account, started_at, &ignore_sync_progress)
                    .await
            });
            started_receiver
                .recv()
                .await
                .expect("first delta request reaches its gate");
            let competing_result = competing_session
                .mail_session()
                .synchronize_account(
                    &account,
                    started_at + Duration::from_secs(1),
                    &ignore_sync_progress,
                )
                .await;
            release_sender
                .send(())
                .await
                .expect("first delta request gate is still open");
            (sync_task.await, competing_result)
        }));

        let report = sync_result.expect("first account synchronization completes");
        assert_eq!(report.folder_count(), 1);
        assert_eq!(
            competing_result,
            Err(MailWorkflowError::OperationInProgress),
            "a competing sync is rejected before it can replace the folder snapshot"
        );
        assert_eq!(folder_list_calls.load(Ordering::Relaxed), 1);
        let folders = competing_session
            .mail_session()
            .list_cached_folders(&account)
            .await
            .expect("saved folder snapshot remains readable");
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].reference(), &folder_reference);
        let messages = competing_session
            .mail_session()
            .list_cached_messages(&folder_reference)
            .await
            .expect("saved message snapshot remains readable");
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0].metadata().subject(),
            Some("Account snapshot survives")
        );
    });
}

#[test]
fn sync_keeps_all_matching_messages_across_the_five_thousand_message_boundary() {
    smol::block_on(async {
        const FIRST_PAGE_COUNT: usize = 5_000;

        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let cutoff = started_at - Duration::from_secs(90 * 24 * 60 * 60);
        let next_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/messages?$skiptoken=synthetic-5001".to_owned(),
        );
        let final_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-complete".to_owned(),
        );
        let first_changes = (0..FIRST_PAGE_COUNT)
            .map(|index| MessageDeltaChange::Upsert {
                id: MessageId::try_from(format!("synthetic-message-{index:04}"))
                    .expect("synthetic message ID is valid"),
                patch: complete_patch(format!("Synthetic message {index}"), started_at),
            })
            .collect();
        let final_changes = vec![
            MessageDeltaChange::Upsert {
                id: MessageId::try_from("synthetic-message-5000".to_owned())
                    .expect("synthetic message ID is valid"),
                patch: complete_patch("Synthetic message 5000".to_owned(), started_at),
            },
            MessageDeltaChange::Upsert {
                id: MessageId::try_from("synthetic-outside-cutoff".to_owned())
                    .expect("synthetic message ID is valid"),
                patch: complete_patch(
                    "Outside cutoff".to_owned(),
                    cutoff - Duration::from_nanos(1),
                ),
            },
        ];
        let provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            [
                Ok(page_response(MessageDeltaPage::new(
                    first_changes,
                    MessageDeltaProgress::Next(next_link.clone()),
                ))),
                Ok(page_response(MessageDeltaPage::new(
                    final_changes,
                    MessageDeltaProgress::Complete(final_link.clone()),
                ))),
            ],
            Err(MailOperationError::ServiceUnavailable),
        );
        let requests = Arc::clone(&provider.requests);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(provider, cache.clone());

        let report = workflow
            .synchronize_account(&account, started_at, &ignore_sync_progress)
            .await
            .expect("both pages commit before the final cursor advances");
        let messages = workflow
            .list_cached_messages(&folder_reference)
            .await
            .expect("all in-scope message metadata is readable");
        let state = workflow
            .cached_sync_state(&folder_reference)
            .await
            .expect("the folder cursor remains readable")
            .expect("the folder cursor was initialized");

        assert_eq!(report.folder_count(), 1);
        assert_eq!(report.page_count(), 2);
        assert_eq!(messages.len(), FIRST_PAGE_COUNT + 1);
        assert!(
            messages
                .iter()
                .all(|message| { message.metadata().received_at() >= &cutoff })
        );
        assert_eq!(state.delta_link(), Some(&final_link));
        assert!(state.next_link().is_none());
        assert_eq!(
            requests
                .lock()
                .expect("synthetic request log mutex is not poisoned")
                .as_slice(),
            [
                MessageDeltaRequest::Initial,
                MessageDeltaRequest::Continue(next_link)
            ]
        );
        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn response_budget_stops_before_committing_an_over_budget_page() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let pages = (0..9).map(|page_index| {
            let next_link = MessageDeltaLink::from_opaque(format!(
                "https://graph.microsoft.com/v1.0/messages?$skiptoken=synthetic-page-{page_index}"
            ));
            Ok(MessageDeltaResponse::Page(MessageDeltaPageResponse::new(
                MessageDeltaPage::new(Vec::new(), MessageDeltaProgress::Next(next_link)),
                8 * 1024 * 1024,
            )))
        });
        let provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            pages,
            Err(MailOperationError::ServiceUnavailable),
        );
        let requests = Arc::clone(&provider.requests);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(provider, cache.clone());

        let error = workflow
            .synchronize_account(
                &account,
                UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                &ignore_sync_progress,
            )
            .await
            .expect_err("response budget prevents unbounded delta traversal");
        assert_eq!(error, MailWorkflowError::ResponseLimitExceeded);

        let state = workflow
            .cached_sync_state(&folder_reference)
            .await
            .expect("last committed checkpoint remains readable")
            .expect("folder sync state exists");
        assert_eq!(
            state.next_link().map(MessageDeltaLink::as_str),
            Some("https://graph.microsoft.com/v1.0/messages?$skiptoken=synthetic-page-7")
        );
        assert_eq!(
            requests.lock().expect("request log mutex is healthy").len(),
            9
        );
        assert!(
            workflow
                .list_cached_messages(&folder_reference)
                .await
                .expect("cached messages remain readable")
                .is_empty()
        );
        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn expired_delta_cursor_rebuild_keeps_cached_mail_until_the_snapshot_commits() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let stale_id =
            MessageId::try_from("stale-message".to_owned()).expect("synthetic message ID is valid");
        let temporary_id = MessageId::try_from("temporary-message".to_owned())
            .expect("synthetic message ID is valid");
        let old_delta_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken=old"
                .to_owned(),
        );
        let reset_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken="
                .to_owned(),
        );
        let next_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$skiptoken=resume"
                .to_owned(),
        );
        let new_delta_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken=new"
                .to_owned(),
        );
        let baseline_page = MessageDeltaPage::new(
            vec![
                MessageDeltaChange::Upsert {
                    id: message_id(),
                    patch: complete_patch("Cached message".to_owned(), started_at),
                },
                MessageDeltaChange::Upsert {
                    id: stale_id.clone(),
                    patch: complete_patch("Stale message".to_owned(), started_at),
                },
            ],
            MessageDeltaProgress::Complete(old_delta_link.clone()),
        );
        let cache = open_cache(&directory, account.clone()).await;
        let baseline_workflow = MailWorkflow::new(
            FixtureProvider::new(
                account.clone(),
                vec![cached_folder.clone()],
                [Ok(page_response(baseline_page))],
                Err(MailOperationError::ServiceUnavailable),
            ),
            cache.clone(),
        );
        baseline_workflow
            .synchronize_account(&account, started_at, &ignore_sync_progress)
            .await
            .expect("baseline snapshot completes");
        cache
            .store_message_body(
                &folder_reference,
                &message_id(),
                MessageBody::new(MessageBodyType::Text, "Synthetic cached body".to_owned()),
            )
            .await
            .expect("body is cached before replacement starts");
        cache
            .store_message_body(
                &folder_reference,
                &stale_id,
                MessageBody::new(MessageBodyType::Text, "Synthetic stale body".to_owned()),
            )
            .await
            .expect("unseen body is cached before replacement starts");
        drop(baseline_workflow);
        cache.close_for_tests().await;

        let cache = open_cache(&directory, account.clone()).await;
        let replacement_page = MessageDeltaPage::new(
            vec![
                MessageDeltaChange::Upsert {
                    id: message_id(),
                    patch: complete_patch("Updated message".to_owned(), started_at),
                },
                MessageDeltaChange::Upsert {
                    id: temporary_id.clone(),
                    patch: complete_patch("Temporary message".to_owned(), started_at),
                },
            ],
            MessageDeltaProgress::Next(next_link.clone()),
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder.clone()],
            [
                Ok(MessageDeltaResponse::Reset(reset_link.clone())),
                Ok(page_response(replacement_page)),
                Err(MailOperationError::ServiceUnavailable),
            ],
            Err(MailOperationError::ServiceUnavailable),
        );
        let requests = Arc::clone(&provider.requests);
        let workflow = MailWorkflow::new(provider, cache.clone());

        assert_eq!(
            workflow
                .synchronize_account(
                    &account,
                    started_at + Duration::from_secs(1),
                    &ignore_sync_progress,
                )
                .await,
            Err(MailWorkflowError::Provider(
                MailOperationError::ServiceUnavailable
            ))
        );
        let interrupted_state = workflow
            .cached_sync_state(&folder_reference)
            .await
            .expect("replacement checkpoint remains readable")
            .expect("folder state remains present");
        assert_eq!(interrupted_state.delta_link(), Some(&old_delta_link));
        assert_eq!(interrupted_state.next_link(), Some(&next_link));
        assert_eq!(interrupted_state.full_sync_generation(), Some(1));
        assert!(interrupted_state.is_rebuilding());
        let visible_before_resume = workflow
            .list_cached_messages(&folder_reference)
            .await
            .expect("old and replacement messages remain visible");
        assert_eq!(visible_before_resume.len(), 3);
        assert!(
            visible_before_resume
                .iter()
                .any(|message| message.id() == &stale_id)
        );
        let (_, cached_body) = cache
            .read_message(&folder_reference, &message_id())
            .await
            .expect("matching message remains readable")
            .expect("matching message remains cached");
        assert!(
            cached_body.is_none(),
            "updated metadata invalidates its body"
        );
        let (_, stale_body) = cache
            .read_message(&folder_reference, &stale_id)
            .await
            .expect("unseen message remains readable during replacement")
            .expect("unseen message remains cached during replacement");
        assert_eq!(
            stale_body
                .expect("unseen body remains until the scan completes")
                .content(),
            "Synthetic stale body"
        );
        assert_eq!(
            requests
                .lock()
                .expect("synthetic request log mutex is healthy")
                .as_slice(),
            [
                MessageDeltaRequest::Continue(old_delta_link),
                MessageDeltaRequest::Continue(reset_link),
                MessageDeltaRequest::Continue(next_link.clone()),
            ]
        );
        drop(workflow);
        cache.close_for_tests().await;

        let cache = open_cache(&directory, account.clone()).await;
        let final_page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Removed { id: temporary_id }],
            MessageDeltaProgress::Complete(new_delta_link.clone()),
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            [Ok(page_response(final_page))],
            Err(MailOperationError::ServiceUnavailable),
        );
        let requests = Arc::clone(&provider.requests);
        let workflow = MailWorkflow::new(provider, cache.clone());
        workflow
            .synchronize_account(
                &account,
                started_at + Duration::from_secs(2),
                &ignore_sync_progress,
            )
            .await
            .expect("replacement scan resumes from its saved page link");

        let completed_state = workflow
            .cached_sync_state(&folder_reference)
            .await
            .expect("replacement state is readable")
            .expect("folder state remains present");
        assert_eq!(completed_state.delta_link(), Some(&new_delta_link));
        assert!(completed_state.next_link().is_none());
        assert!(!completed_state.is_rebuilding());
        let visible_after_resume = workflow
            .list_cached_messages(&folder_reference)
            .await
            .expect("completed snapshot is readable");
        assert_eq!(visible_after_resume.len(), 1);
        assert_eq!(visible_after_resume[0].id(), &message_id());
        let (_, cached_body) = cache
            .read_message(&folder_reference, &message_id())
            .await
            .expect("matching message remains readable after replacement")
            .expect("matching message remains cached");
        assert!(
            cached_body.is_none(),
            "replacement survivors require a fresh body"
        );
        assert_eq!(
            requests
                .lock()
                .expect("synthetic request log mutex is healthy")
                .as_slice(),
            [MessageDeltaRequest::Continue(next_link)]
        );
        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn a_second_delta_reset_is_checkpointed_for_the_next_attempt() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let first_reset = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken=first"
                .to_owned(),
        );
        let second_reset = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken=second"
                .to_owned(),
        );
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(
            FixtureProvider::new(
                account.clone(),
                vec![cached_folder.clone()],
                [
                    Ok(MessageDeltaResponse::Reset(first_reset.clone())),
                    Ok(MessageDeltaResponse::Reset(second_reset.clone())),
                ],
                Err(MailOperationError::ServiceUnavailable),
            ),
            cache.clone(),
        );

        assert_eq!(
            workflow
                .synchronize_account(
                    &account,
                    UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                    &ignore_sync_progress,
                )
                .await,
            Err(MailWorkflowError::RepeatedDeltaReset)
        );
        let state = workflow
            .cached_sync_state(&folder_reference)
            .await
            .expect("latest reset checkpoint remains readable")
            .expect("folder state exists");
        assert_eq!(state.next_link(), Some(&second_reset));
        assert_eq!(state.full_sync_generation(), Some(2));
        assert!(state.is_rebuilding());
        drop(workflow);
        cache.close_for_tests().await;

        let cache = open_cache(&directory, account.clone()).await;
        let completed_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken=complete"
                .to_owned(),
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            [Ok(page_response(MessageDeltaPage::new(
                Vec::new(),
                MessageDeltaProgress::Complete(completed_link),
            )))],
            Err(MailOperationError::ServiceUnavailable),
        );
        let requests = Arc::clone(&provider.requests);
        let workflow = MailWorkflow::new(provider, cache.clone());
        workflow
            .synchronize_account(
                &account,
                UNIX_EPOCH + Duration::from_secs(1_800_000_001),
                &ignore_sync_progress,
            )
            .await
            .expect("the next attempt resumes the latest reset checkpoint");
        assert_eq!(
            requests
                .lock()
                .expect("synthetic request log mutex is healthy")
                .as_slice(),
            [MessageDeltaRequest::Continue(second_reset)]
        );
        let state = workflow
            .cached_sync_state(&folder_reference)
            .await
            .expect("completed checkpoint remains readable")
            .expect("folder state exists");
        assert!(!state.is_rebuilding());
        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn stale_delta_responses_cannot_advance_a_replaced_checkpoint() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let cutoff = started_at - Duration::from_secs(90 * 24 * 60 * 60);
        let cache = open_cache(&directory, account.clone()).await;
        cache
            .replace_folders(&account, vec![cached_folder])
            .await
            .expect("synthetic folder is cached");
        cache
            .ensure_account_sync_scope(&account, cutoff)
            .await
            .expect("account cutoff is saved");
        let initial_state = cache
            .ensure_sync_state(&folder_reference, cutoff)
            .await
            .expect("initial checkpoint is saved");
        let old_delta_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken=old"
                .to_owned(),
        );
        let baseline_page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Baseline message".to_owned(), started_at),
            }],
            MessageDeltaProgress::Complete(old_delta_link),
        );
        let baseline_state = cache
            .commit_delta_page(&folder_reference, &initial_state, baseline_page, started_at)
            .await
            .expect("baseline snapshot commits");
        let reset_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken="
                .to_owned(),
        );
        let replacement_state = cache
            .begin_full_resync(&folder_reference, &baseline_state, reset_link)
            .await
            .expect("replacement checkpoint starts");

        assert_eq!(
            cache
                .begin_full_resync(
                    &folder_reference,
                    &baseline_state,
                    MessageDeltaLink::from_opaque(
                        "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken=stale-reset"
                            .to_owned(),
                    ),
                )
                .await,
            Err(MailCacheError::SyncStateChanged)
        );
        assert_eq!(
            cache
                .commit_delta_page(
                    &folder_reference,
                    &baseline_state,
                    MessageDeltaPage::new(
                        vec![MessageDeltaChange::Upsert {
                            id: MessageId::try_from("stale-session-message".to_owned())
                                .expect("synthetic message ID is valid"),
                            patch: complete_patch("Stale page".to_owned(), started_at),
                        }],
                        MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken=stale-page"
                                .to_owned(),
                        )),
                    ),
                    started_at + Duration::from_secs(1),
                )
                .await,
            Err(MailCacheError::SyncStateChanged)
        );

        let stored_state = cache
            .load_sync_state(&folder_reference)
            .await
            .expect("replacement checkpoint remains readable")
            .expect("folder checkpoint exists");
        assert_eq!(stored_state, replacement_state);
        assert_eq!(stored_state.checkpoint_revision(), 2);
        assert!(stored_state.is_rebuilding());
        let messages = cache
            .list_messages(&folder_reference)
            .await
            .expect("the previous snapshot remains readable");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id(), &message_id());
        cache.close_for_tests().await;
    });
}

#[test]
fn response_budget_remains_cumulative_after_a_delta_reset() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let reset_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken="
                .to_owned(),
        );
        let pages = (0..8)
            .map(|page_index| {
                let next_link = MessageDeltaLink::from_opaque(format!(
                    "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$skiptoken=before-reset-{page_index}"
                ));
                Ok(MessageDeltaResponse::Page(MessageDeltaPageResponse::new(
                    MessageDeltaPage::new(Vec::new(), MessageDeltaProgress::Next(next_link)),
                    8 * 1024 * 1024,
                )))
            })
            .chain([
                Ok(MessageDeltaResponse::Reset(reset_link.clone())),
                Ok(MessageDeltaResponse::Page(MessageDeltaPageResponse::new(
                    MessageDeltaPage::new(
                        Vec::new(),
                        MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken=complete"
                                .to_owned(),
                        )),
                    ),
                    1,
                ))),
            ]);
        let provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            pages,
            Err(MailOperationError::ServiceUnavailable),
        );
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(provider, cache.clone());

        assert_eq!(
            workflow
                .synchronize_account(
                    &account,
                    UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                    &ignore_sync_progress,
                )
                .await,
            Err(MailWorkflowError::ResponseLimitExceeded)
        );
        let state = workflow
            .cached_sync_state(&folder_reference)
            .await
            .expect("reset checkpoint remains readable")
            .expect("folder state exists");
        assert_eq!(state.next_link(), Some(&reset_link));
        assert!(state.is_rebuilding());
        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn stale_cache_handle_cannot_stage_a_send_after_account_cache_removal() {
    smol::block_on(async {
        let directory = TempDir::new().expect("synthetic mail cache directory is created");
        let account = account_id();
        let removal_cache = open_cache(&directory, account.clone()).await;
        let stale_send_cache = open_cache(&directory, account.clone()).await;
        let _lock = security::lock_account_directory(&directory.path().join("mail.lock"))
            .await
            .expect("synthetic cache lifecycle lock is acquired");
        removal_cache.close_for_tests().await;
        std::fs::remove_file(directory.path().join("mail.sqlite3"))
            .expect("cache removal unlinks the database before the stale handle can stage a send");
        drop(_lock);

        let provider = FixtureProvider::new(
            account.clone(),
            Vec::new(),
            [],
            Err(MailOperationError::ServiceUnavailable),
        )
        .with_send_result(Ok(()));
        let send_calls = Arc::clone(&provider.send_calls);
        let workflow = MailWorkflow::new(provider, stale_send_cache.clone());

        assert_eq!(
            workflow
                .send_message(
                    &account,
                    draft("Stale cache subject", "stale cache body"),
                    UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                )
                .await,
            Err(MailSendWorkflowError::Cache(MailCacheError::Unavailable))
        );
        assert_eq!(send_calls.load(Ordering::Relaxed), 0);

        drop(workflow);
        removal_cache.close_for_tests().await;
        stale_send_cache.close_for_tests().await;
    });
}

#[test]
fn unknown_send_marker_survives_encrypted_restart_and_blocks_duplicate_send_and_removal() {
    smol::block_on(async {
        let directory = TempDir::new().expect("synthetic mail cache directory is created");
        let account = account_id();
        let provider = FixtureProvider::new(
            account.clone(),
            Vec::new(),
            [],
            Err(MailOperationError::ServiceUnavailable),
        )
        .with_send_result(Err(MailSendProviderError::OutcomeUnknown));
        let send_calls = Arc::clone(&provider.send_calls);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(provider.clone(), cache.clone());
        let result = workflow
            .send_message(
                &account,
                draft("Synthetic subject", "synthetic-private-body"),
                UNIX_EPOCH + Duration::from_secs(1_800_000_000),
            )
            .await
            .expect("ambiguous submission is represented as an outcome");
        let pending = match result {
            MailSendOutcome::OutcomeUnknown(pending) => pending,
            other => panic!("expected unresolved send marker, got {other:?}"),
        };
        let removal_cache = open_cache(&directory, account.clone()).await;

        assert_eq!(send_calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            <SqliteMailCache as MailCache>::remove_account(&removal_cache, &account).await,
            Err(MailCacheError::SendAlreadyPending)
        );
        assert_eq!(
            <SqliteMailCache as einfach_mail_application::CalendarCache>::remove_account(
                &removal_cache,
                &account,
            )
            .await,
            Err(CalendarCacheError::PendingMailSend)
        );
        assert_eq!(
            workflow.load_pending_send(&account).await,
            Ok(Some(pending.clone()))
        );
        drop(workflow);
        removal_cache.close_for_tests().await;
        cache.close_for_tests().await;

        let encrypted_bytes = std::fs::read(directory.path().join("mail.sqlite3"))
            .expect("SQLCipher database remains on disk");
        assert!(
            !encrypted_bytes
                .windows(b"synthetic-private-body".len())
                .any(|window| window == b"synthetic-private-body")
        );

        let reopened = open_cache(&directory, account.clone()).await;
        assert_eq!(
            reopened.load_pending_send(&account).await,
            Ok(Some(pending.clone()))
        );
        let workflow = MailWorkflow::new(provider, reopened.clone());
        let duplicate = workflow
            .send_message(
                &account,
                draft("Another synthetic subject", "another-body"),
                UNIX_EPOCH + Duration::from_secs(1_800_000_001),
            )
            .await;
        assert!(matches!(
            duplicate,
            Err(MailSendWorkflowError::PendingResolutionRequired(ref saved))
                if saved == &pending
        ));
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);

        workflow
            .resolve_pending_send(&account, pending.operation_id())
            .await
            .expect("explicit resolution clears the old marker without resending");
        assert_eq!(workflow.load_pending_send(&account).await, Ok(None));
        drop(workflow);
        reopened.close_for_tests().await;
    });
}

#[test]
fn accepted_and_definitively_rejected_sends_clear_their_markers() {
    smol::block_on(async {
        let directory = TempDir::new().expect("synthetic mail cache directory is created");
        let account = account_id();
        let cache = open_cache(&directory, account.clone()).await;
        let accepted_provider = FixtureProvider::new(
            account.clone(),
            Vec::new(),
            [],
            Err(MailOperationError::ServiceUnavailable),
        )
        .with_send_result(Ok(()));
        let accepted_calls = Arc::clone(&accepted_provider.send_calls);
        let accepted_workflow = MailWorkflow::new(accepted_provider, cache.clone());

        assert_eq!(
            accepted_workflow
                .send_message(
                    &account,
                    draft("Accepted synthetic subject", "accepted synthetic body"),
                    UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                )
                .await,
            Ok(MailSendOutcome::Accepted)
        );
        assert_eq!(
            accepted_workflow.load_pending_send(&account).await,
            Ok(None)
        );
        assert_eq!(accepted_calls.load(Ordering::Relaxed), 1);
        drop(accepted_workflow);

        let rejected_provider = FixtureProvider::new(
            account.clone(),
            Vec::new(),
            [],
            Err(MailOperationError::ServiceUnavailable),
        )
        .with_send_result(Err(MailSendProviderError::Rejected));
        let rejected_calls = Arc::clone(&rejected_provider.send_calls);
        let rejected_workflow = MailWorkflow::new(rejected_provider, cache.clone());

        assert_eq!(
            rejected_workflow
                .send_message(
                    &account,
                    draft("Rejected synthetic subject", "rejected synthetic body"),
                    UNIX_EPOCH + Duration::from_secs(1_800_000_001),
                )
                .await,
            Err(MailSendWorkflowError::Provider(
                MailSendProviderError::Rejected
            ))
        );
        assert_eq!(
            rejected_workflow.load_pending_send(&account).await,
            Ok(None)
        );
        assert_eq!(rejected_calls.load(Ordering::Relaxed), 1);
        drop(rejected_workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn rejected_send_with_marker_clear_failure_keeps_a_resolvable_known_rejection() {
    smol::block_on(async {
        let directory = TempDir::new().expect("synthetic mail cache directory is created");
        let account = account_id();
        let cache = open_cache(&directory, account.clone()).await;
        cache.fail_next_pending_send_clear();
        let provider = FixtureProvider::new(
            account.clone(),
            Vec::new(),
            [],
            Err(MailOperationError::ServiceUnavailable),
        )
        .with_send_result(Err(MailSendProviderError::Rejected));
        let send_calls = Arc::clone(&provider.send_calls);
        let workflow = MailWorkflow::new(provider, cache.clone());

        let pending = match workflow
            .send_message(
                &account,
                draft("Rejected synthetic subject", "rejected synthetic body"),
                UNIX_EPOCH + Duration::from_secs(1_800_000_000),
            )
            .await
        {
            Err(MailSendWorkflowError::RejectedNeedsResolution {
                pending,
                provider: MailSendProviderError::Rejected,
                cache: MailCacheError::Unavailable,
            }) => pending,
            other => panic!("expected the known rejection and its retained marker, got {other:?}"),
        };

        assert_eq!(
            workflow.load_pending_send(&account).await,
            Ok(Some(pending.clone()))
        );
        workflow
            .resolve_pending_send(&account, pending.operation_id())
            .await
            .expect("explicit recovery clears the known rejection without resending");
        assert_eq!(workflow.load_pending_send(&account).await, Ok(None));
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);

        drop(workflow);
        cache.close_for_tests().await;
    });
}
