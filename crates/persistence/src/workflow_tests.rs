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
    MailOperationError, MailProvider, MailWorkflow, MailWorkflowError, MessageDeltaPageResponse,
    MessageDeltaRequest,
};
use einfach_mail_domain::{
    AccountId, FolderId, FolderReference, MailFolder, MailMessage, MessageBody, MessageBodyType,
    MessageDeltaChange, MessageDeltaLink, MessageDeltaPage, MessageDeltaProgress, MessageId,
    MessageMetadataPatch, ReadState,
};
use tempfile::TempDir;

use crate::store::SqliteMailCache;

const DATABASE_KEY_BYTE: u8 = 0x62;

type PageQueue = VecDeque<Result<MessageDeltaPageResponse, MailOperationError>>;

struct FixtureProvider {
    account_id: AccountId,
    folders: Vec<MailFolder>,
    pages: Arc<Mutex<PageQueue>>,
    requests: Arc<Mutex<Vec<MessageDeltaRequest>>>,
    body_result: Result<MessageBody, MailOperationError>,
    body_fetches: Arc<AtomicUsize>,
}

impl FixtureProvider {
    fn new(
        account_id: AccountId,
        folders: Vec<MailFolder>,
        pages: impl IntoIterator<Item = Result<MessageDeltaPageResponse, MailOperationError>>,
        body_result: Result<MessageBody, MailOperationError>,
    ) -> Self {
        Self {
            account_id,
            folders,
            pages: Arc::new(Mutex::new(pages.into_iter().collect())),
            requests: Arc::new(Mutex::new(Vec::new())),
            body_result,
            body_fetches: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl MailProvider for FixtureProvider {
    fn list_folders(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<Vec<MailFolder>, MailOperationError>> + Send + '_ {
        let folders = self.folders.clone();
        let matches_account = account_id == &self.account_id;
        async move {
            if matches_account {
                Ok(folders)
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
    ) -> impl Future<Output = Result<MessageDeltaPageResponse, MailOperationError>> + Send + '_
    {
        let account_id = folder.account_id().clone();
        let expected_account = self.account_id.clone();
        let pages = Arc::clone(&self.pages);
        let requests = Arc::clone(&self.requests);
        async move {
            if account_id != expected_account {
                return Err(MailOperationError::AccountMismatch);
            }
            requests
                .lock()
                .expect("synthetic request log mutex is not poisoned")
                .push(request);
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
        async move {
            if account_id != expected_account {
                return Err(MailOperationError::AccountMismatch);
            }
            fetches.fetch_add(1, Ordering::Relaxed);
            result
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

fn page_response(page: MessageDeltaPage) -> MessageDeltaPageResponse {
    MessageDeltaPageResponse::new(page, 1_024)
}

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
                "<p>Synthetic body</p>".to_owned(),
            )),
        );
        let initial_requests = Arc::clone(&initial_provider.requests);
        let initial_body_fetches = Arc::clone(&initial_provider.body_fetches);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(initial_provider, cache.clone());

        assert_eq!(
            workflow.synchronize_account(&account, started_at).await,
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
                "<p>Synthetic body</p>".to_owned(),
            )),
        );
        let resume_requests = Arc::clone(&resume_provider.requests);
        let body_fetches = Arc::clone(&resume_provider.body_fetches);
        let resumed_workflow = MailWorkflow::new(resume_provider, cache.clone());
        let report = resumed_workflow
            .synchronize_account(&account, started_at + Duration::from_secs(86_400))
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
        assert_eq!(opened.body().content(), "<p>Synthetic body</p>");
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
        assert_eq!(reopened_message.body().content(), "<p>Synthetic body</p>");
        assert_eq!(offline_fetches.load(Ordering::Relaxed), 0);
        drop(offline_workflow);
        reopened_cache.close_for_tests().await;
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
            .synchronize_account(&account, started_at)
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
            Ok(MessageDeltaPageResponse::new(
                MessageDeltaPage::new(Vec::new(), MessageDeltaProgress::Next(next_link)),
                8 * 1024 * 1024,
            ))
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
            .synchronize_account(&account, UNIX_EPOCH + Duration::from_secs(1_800_000_000))
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
