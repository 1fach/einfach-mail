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
    MessageDeltaResponse, WellKnownMailFolder,
};
use einfach_mail_domain::{
    AccountId, CalendarEventCreation, CalendarEventUpdate, CalendarOccurrence, CalendarViewRange,
    CalendarViewReference, EditableCalendarEvent, FolderId, FolderReference, FollowUpFlagStatus,
    MailDraft, MailFolder, MailMessage, MailRecipient, MailReplyDraft, MailReplySource,
    MailSubject, MessageBody, MessageBodyType, MessageDeltaChange, MessageDeltaLink,
    MessageDeltaPage, MessageDeltaProgress, MessageId, MessageMetadataPatch, ReadState,
};
use tempfile::TempDir;
use time::{Date, Month};

use crate::store::SqliteMailCache;

const DATABASE_KEY_BYTE: u8 = 0x62;

type PageQueue = VecDeque<Result<MessageDeltaResponse, MailOperationError>>;
type ProviderGate = Arc<Mutex<Option<(smol::channel::Sender<()>, smol::channel::Receiver<()>)>>>;
type ReadStateGate = Arc<Mutex<Option<(smol::channel::Sender<()>, smol::channel::Receiver<()>)>>>;
type FollowUpFlagGate =
    Arc<Mutex<Option<(smol::channel::Sender<()>, smol::channel::Receiver<()>)>>>;

#[derive(Clone, Copy)]
enum FollowUpFlagResponse {
    Confirm,
    Deny,
    Unknown,
}

#[derive(Clone)]
struct FixtureProvider {
    account_id: AccountId,
    folders: Vec<MailFolder>,
    folder_list_gate: Option<(smol::channel::Sender<()>, smol::channel::Receiver<()>)>,
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
    follow_up_flag_updates: Arc<Mutex<Vec<(MessageId, FollowUpFlagStatus)>>>,
    follow_up_flag_gate: FollowUpFlagGate,
    follow_up_flag_response: FollowUpFlagResponse,
    message_moves: Arc<Mutex<Vec<(FolderReference, MessageId, FolderReference)>>>,
    move_result: Arc<Mutex<Result<(), MailOperationError>>>,
    move_gate: ProviderGate,
    archive_folder_id: FolderId,
    deleted_items_folder_id: FolderId,
    well_known_folder_calls: Arc<AtomicUsize>,
    well_known_folder_gate: ProviderGate,
    send_gate: ProviderGate,
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
            folder_list_gate: None,
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
            follow_up_flag_updates: Arc::new(Mutex::new(Vec::new())),
            follow_up_flag_gate: Arc::new(Mutex::new(None)),
            follow_up_flag_response: FollowUpFlagResponse::Confirm,
            message_moves: Arc::new(Mutex::new(Vec::new())),
            move_result: Arc::new(Mutex::new(Ok(()))),
            move_gate: Arc::new(Mutex::new(None)),
            archive_folder_id: FolderId::try_from("folder-archive".to_owned())
                .expect("synthetic Archive ID is valid"),
            deleted_items_folder_id: FolderId::try_from("deleted-items".to_owned())
                .expect("synthetic Deleted Items ID is valid"),
            well_known_folder_calls: Arc::new(AtomicUsize::new(0)),
            well_known_folder_gate: Arc::new(Mutex::new(None)),
            send_gate: Arc::new(Mutex::new(None)),
            calendar_gate: None,
        }
    }

    fn hold_next_submission(&self) -> (smol::channel::Receiver<()>, smol::channel::Sender<()>) {
        let (started_tx, started_rx) = smol::channel::bounded(1);
        let (release_tx, release_rx) = smol::channel::bounded(1);
        *self
            .send_gate
            .lock()
            .expect("synthetic send gate mutex is healthy") = Some((started_tx, release_rx));
        (started_rx, release_tx)
    }

    fn hold_next_message_move(&self) -> (smol::channel::Receiver<()>, smol::channel::Sender<()>) {
        let (started_tx, started_rx) = smol::channel::bounded(1);
        let (release_tx, release_rx) = smol::channel::bounded(1);
        *self
            .move_gate
            .lock()
            .expect("synthetic move gate mutex is healthy") = Some((started_tx, release_rx));
        (started_rx, release_tx)
    }

    fn hold_next_well_known_folder_lookup(
        &self,
    ) -> (smol::channel::Receiver<()>, smol::channel::Sender<()>) {
        let (started_tx, started_rx) = smol::channel::bounded(1);
        let (release_tx, release_rx) = smol::channel::bounded(1);
        *self
            .well_known_folder_gate
            .lock()
            .expect("synthetic folder lookup gate mutex is healthy") =
            Some((started_tx, release_rx));
        (started_rx, release_tx)
    }

    fn hold_next_read_state(&self) -> (smol::channel::Receiver<()>, smol::channel::Sender<()>) {
        let (started_tx, started_rx) = smol::channel::bounded(1);
        let (release_tx, release_rx) = smol::channel::bounded(1);
        *self
            .read_state_gate
            .lock()
            .expect("synthetic read-state gate mutex is healthy") = Some((started_tx, release_rx));
        (started_rx, release_tx)
    }

    fn hold_next_follow_up_flag(&self) -> (smol::channel::Receiver<()>, smol::channel::Sender<()>) {
        let (started_tx, started_rx) = smol::channel::bounded(1);
        let (release_tx, release_rx) = smol::channel::bounded(1);
        *self
            .follow_up_flag_gate
            .lock()
            .expect("synthetic flag gate mutex is healthy") = Some((started_tx, release_rx));
        (started_rx, release_tx)
    }

    fn with_send_result(mut self, result: Result<(), MailSendProviderError>) -> Self {
        self.send_result = result;
        self
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
        let folder_list_gate = self.folder_list_gate.clone();
        async move {
            if matches_account {
                if let Some((started, release)) = folder_list_gate {
                    started
                        .send(())
                        .await
                        .map_err(|_| MailOperationError::ServiceUnavailable)?;
                    release
                        .recv()
                        .await
                        .map_err(|_| MailOperationError::ServiceUnavailable)?;
                }
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

    fn set_message_follow_up_flag_status(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
        status: FollowUpFlagStatus,
    ) -> impl Future<Output = Result<FollowUpFlagStatus, MailOperationError>> + Send + '_ {
        let matches_account = folder.account_id() == &self.account_id;
        let message_id = message_id.clone();
        let updates = Arc::clone(&self.follow_up_flag_updates);
        let gate = Arc::clone(&self.follow_up_flag_gate);
        let response = self.follow_up_flag_response;
        async move {
            if !matches_account {
                return Err(MailOperationError::AccountMismatch);
            }
            let gate = gate
                .lock()
                .expect("fixture flag gate mutex is healthy")
                .take();
            if let Some((started, release)) = gate {
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
                .expect("fixture flag update log mutex is healthy")
                .push((message_id, status));
            match response {
                FollowUpFlagResponse::Confirm => Ok(status),
                FollowUpFlagResponse::Deny => Err(MailOperationError::AccessDenied),
                FollowUpFlagResponse::Unknown => {
                    Err(MailOperationError::MessageUpdateOutcomeUnknown)
                }
            }
        }
    }

    fn move_message(
        &self,
        source_folder: &FolderReference,
        message_id: &MessageId,
        destination_folder: &FolderReference,
    ) -> impl Future<Output = Result<(), MailOperationError>> + Send + '_ {
        let account_matches = source_folder.account_id() == &self.account_id
            && destination_folder.account_id() == &self.account_id;
        let request = (
            source_folder.clone(),
            message_id.clone(),
            destination_folder.clone(),
        );
        let message_moves = Arc::clone(&self.message_moves);
        let result = Arc::clone(&self.move_result);
        let gate = Arc::clone(&self.move_gate);
        async move {
            if !account_matches {
                return Err(MailOperationError::AccountMismatch);
            }
            message_moves
                .lock()
                .expect("synthetic message-move log mutex is not poisoned")
                .push(request);
            let gate = gate
                .lock()
                .expect("synthetic move gate mutex is healthy")
                .take();
            if let Some((started, release)) = gate {
                let _ = started.send(()).await;
                let _ = release.recv().await;
            }
            *result
                .lock()
                .expect("synthetic message-move result mutex is not poisoned")
        }
    }

    fn well_known_folder(
        &self,
        account_id: &AccountId,
        folder: WellKnownMailFolder,
    ) -> impl Future<Output = Result<FolderReference, MailOperationError>> + Send + '_ {
        let account_matches = account_id == &self.account_id;
        let folder_id = match folder {
            WellKnownMailFolder::Archive => self.archive_folder_id.clone(),
            WellKnownMailFolder::DeletedItems => self.deleted_items_folder_id.clone(),
        };
        let folder_reference = FolderReference::new(account_id.clone(), folder_id);
        let calls = Arc::clone(&self.well_known_folder_calls);
        let gate = Arc::clone(&self.well_known_folder_gate);
        async move {
            if !account_matches {
                return Err(MailOperationError::AccountMismatch);
            }
            calls.fetch_add(1, Ordering::Relaxed);
            let gate = gate
                .lock()
                .expect("synthetic folder lookup gate mutex is healthy")
                .take();
            if let Some((started, release)) = gate {
                started
                    .send(())
                    .await
                    .map_err(|_| MailOperationError::ServiceUnavailable)?;
                release
                    .recv()
                    .await
                    .map_err(|_| MailOperationError::ServiceUnavailable)?;
            }
            Ok(folder_reference)
        }
    }

    fn send_mail(
        &self,
        account_id: &AccountId,
        _draft: MailDraft,
    ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_ {
        let matches_account = account_id == &self.account_id;
        let calls = Arc::clone(&self.send_calls);
        let gate = Arc::clone(&self.send_gate);
        let result = self.send_result;
        async move {
            if !matches_account {
                return Err(MailSendProviderError::AccountMismatch);
            }
            calls.fetch_add(1, Ordering::Relaxed);
            let gate = gate
                .lock()
                .expect("synthetic send gate mutex is healthy")
                .take();
            if let Some((started, release)) = gate {
                let _ = started.send(()).await;
                let _ = release.recv().await;
            }
            result
        }
    }

    fn reply_to_message(
        &self,
        account_id: &AccountId,
        draft: einfach_mail_domain::MailReplyDraft,
    ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_ {
        let matches_account =
            account_id == &self.account_id && draft.source().account_id() == &self.account_id;
        let calls = Arc::clone(&self.send_calls);
        let gate = Arc::clone(&self.send_gate);
        let result = self.send_result;
        async move {
            if !matches_account {
                return Err(MailSendProviderError::AccountMismatch);
            }
            calls.fetch_add(1, Ordering::Relaxed);
            let gate = gate
                .lock()
                .expect("synthetic send gate mutex is healthy")
                .take();
            if let Some((started, release)) = gate {
                let _ = started.send(()).await;
                let _ = release.recv().await;
            }
            result
        }
    }
}

impl CalendarProvider for FixtureProvider {
    fn editable_event(
        &self,
        _: &CalendarOccurrence,
    ) -> impl Future<Output = Result<EditableCalendarEvent, CalendarOperationError>> + Send + '_
    {
        ready(Err(CalendarOperationError::ServiceUnavailable))
    }

    fn create_event(
        &self,
        _: &CalendarEventCreation,
    ) -> impl Future<Output = Result<CalendarOccurrence, CalendarOperationError>> + Send + '_ {
        ready(Err(CalendarOperationError::ServiceUnavailable))
    }

    fn update_event(
        &self,
        _: &CalendarEventUpdate,
    ) -> impl Future<Output = Result<CalendarOccurrence, CalendarOperationError>> + Send + '_ {
        ready(Err(CalendarOperationError::ServiceUnavailable))
    }

    fn delete_event(
        &self,
        _: &EditableCalendarEvent,
    ) -> impl Future<Output = Result<(), CalendarOperationError>> + Send + '_ {
        ready(Err(CalendarOperationError::ServiceUnavailable))
    }

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

fn archive_folder(account_id: &AccountId) -> MailFolder {
    let reference = FolderReference::new(
        account_id.clone(),
        FolderId::try_from("folder-archive".to_owned()).expect("synthetic folder ID is valid"),
    );
    MailFolder::new(reference, None, "Synthetic Archive".to_owned())
        .expect("synthetic archive folder is valid")
}

fn deleted_items_folder(account_id: &AccountId) -> MailFolder {
    let reference = FolderReference::new(
        account_id.clone(),
        FolderId::try_from("deleted-items".to_owned()).expect("synthetic folder ID is valid"),
    );
    MailFolder::new(reference, None, "Gelöschte Elemente".to_owned())
        .expect("synthetic Deleted Items folder is valid")
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

fn completed_message_delta_page(scope: &str, changes: Vec<MessageDeltaChange>) -> MessageDeltaPage {
    MessageDeltaPage::new(
        changes,
        MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(format!(
            "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-{scope}"
        ))),
    )
}

fn page_response(page: MessageDeltaPage) -> MessageDeltaResponse {
    MessageDeltaResponse::Page(MessageDeltaPageResponse::new(page, 1_024))
}

fn provider_with_cached_message(
    account: AccountId,
    folders: Vec<MailFolder>,
    started_at: SystemTime,
    body: Result<MessageBody, MailOperationError>,
) -> FixtureProvider {
    let cutoff = started_at - Duration::from_secs(90 * 24 * 60 * 60);
    let page = MessageDeltaPage::new(
        vec![MessageDeltaChange::Upsert {
            id: message_id(),
            patch: complete_patch("Synthetic subject".to_owned(), cutoff),
        }],
        MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-account-removal"
                .to_owned(),
        )),
    );
    FixtureProvider::new(account, folders, [Ok(page_response(page))], body)
}

fn ignore_sync_progress(_: MailSyncProgress) {}

fn assert_removal_completed(result: Result<(), MailCacheError>, database_path: &std::path::Path) {
    assert_ne!(result, Err(MailCacheError::OperationInProgress));
    assert!(!database_path.exists());
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
fn follow_up_flag_write_reserves_folder_until_confirmation_and_atomic_cache_commit() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut patch = complete_patch("Synthetic message".to_owned(), started_at);
        patch.set_follow_up_flag_status(Some(FollowUpFlagStatus::NotFlagged));
        let page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch,
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
        let (update_started, release_update) = provider.hold_next_follow_up_flag();
        let updates = Arc::clone(&provider.follow_up_flag_updates);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = Arc::new(MailWorkflow::new(provider, cache.clone()));
        workflow
            .synchronize_account(&account, started_at, &ignore_sync_progress)
            .await
            .expect("synthetic message is cached before its flag action");

        let executor = smol::Executor::new();
        let update_workflow = Arc::clone(&workflow);
        let update_folder = folder_reference.clone();
        let (update_result, sync_result, staged_marker) = smol::block_on(executor.run(async {
            let update_task = executor.spawn(async move {
                update_workflow
                    .set_message_follow_up_flag_status(
                        &update_folder,
                        &message_id(),
                        FollowUpFlagStatus::Flagged,
                    )
                    .await
            });
            update_started
                .recv()
                .await
                .expect("synthetic provider waits before confirming the flag write");
            let staged_marker = cache
                .folder_write_sync_required(&folder_reference)
                .await
                .expect("recovery marker is readable while the PATCH is pending");
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
            (update_task.await, sync_result, staged_marker)
        }));

        assert!(staged_marker);
        assert_eq!(sync_result, Err(MailWorkflowError::OperationInProgress));
        assert_eq!(
            update_result.expect("flag update completes"),
            einfach_mail_application::FollowUpFlagUpdateOutcome::ConfirmedAndCached(
                FollowUpFlagStatus::Flagged
            )
        );
        assert!(
            !cache
                .folder_write_sync_required(&folder_reference)
                .await
                .expect("confirmed cache update clears its recovery marker")
        );
        assert_eq!(
            cache
                .read_message(&folder_reference, &message_id())
                .await
                .expect("updated message remains readable")
                .and_then(|(message, _)| message.metadata().follow_up_flag_status()),
            Some(FollowUpFlagStatus::Flagged)
        );
        assert_eq!(
            updates
                .lock()
                .expect("synthetic flag update log mutex is healthy")
                .as_slice(),
            [(message_id(), FollowUpFlagStatus::Flagged)]
        );

        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn definite_follow_up_flag_rejection_clears_marker_without_changing_cached_status() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut patch = complete_patch("Synthetic message".to_owned(), started_at);
        patch.set_follow_up_flag_status(Some(FollowUpFlagStatus::NotFlagged));
        let page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch,
            }],
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-initial"
                    .to_owned(),
            )),
        );
        let mut provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            [Ok(page_response(page))],
            Err(MailOperationError::ServiceUnavailable),
        );
        provider.follow_up_flag_response = FollowUpFlagResponse::Deny;
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(provider, cache.clone());
        workflow
            .synchronize_account(&account, started_at, &ignore_sync_progress)
            .await
            .expect("synthetic message is cached before its flag action");

        assert_eq!(
            workflow
                .set_message_follow_up_flag_status(
                    &folder_reference,
                    &message_id(),
                    FollowUpFlagStatus::Flagged,
                )
                .await,
            Err(MailWorkflowError::Provider(
                MailOperationError::AccessDenied
            ))
        );
        assert!(
            !cache
                .folder_write_sync_required(&folder_reference)
                .await
                .expect("definite rejection clears its recovery marker")
        );
        assert_eq!(
            cache
                .read_message(&folder_reference, &message_id())
                .await
                .expect("cached state remains readable after rejection")
                .and_then(|(message, _)| message.metadata().follow_up_flag_status()),
            Some(FollowUpFlagStatus::NotFlagged)
        );

        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn unknown_follow_up_flag_outcome_survives_reopen_until_a_complete_delta_round() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut patch = complete_patch("Synthetic message".to_owned(), started_at);
        patch.set_follow_up_flag_status(Some(FollowUpFlagStatus::NotFlagged));
        let initial_page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch,
            }],
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-initial"
                    .to_owned(),
            )),
        );
        let mut provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder.clone()],
            [Ok(page_response(initial_page))],
            Err(MailOperationError::ServiceUnavailable),
        );
        provider.follow_up_flag_response = FollowUpFlagResponse::Unknown;
        let updates = Arc::clone(&provider.follow_up_flag_updates);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(provider, cache.clone());
        workflow
            .synchronize_account(&account, started_at, &ignore_sync_progress)
            .await
            .expect("synthetic message is cached before its flag action");

        assert_eq!(
            workflow
                .set_message_follow_up_flag_status(
                    &folder_reference,
                    &message_id(),
                    FollowUpFlagStatus::Flagged,
                )
                .await,
            Err(MailWorkflowError::Provider(
                MailOperationError::MessageUpdateOutcomeUnknown
            ))
        );
        assert!(
            cache
                .folder_write_sync_required(&folder_reference)
                .await
                .expect("unknown outcome keeps the recovery marker")
        );
        assert_eq!(
            workflow
                .set_message_follow_up_flag_status(
                    &folder_reference,
                    &message_id(),
                    FollowUpFlagStatus::Flagged,
                )
                .await,
            Err(MailWorkflowError::FolderSyncRequired)
        );
        assert_eq!(
            updates
                .lock()
                .expect("synthetic flag update log mutex is healthy")
                .len(),
            1
        );
        drop(workflow);
        cache.close_for_tests().await;

        let reopened = open_cache(&directory, account.clone()).await;
        assert!(
            reopened
                .folder_write_sync_required(&folder_reference)
                .await
                .expect("recovery marker survives cache reopen")
        );
        let final_page = MessageDeltaPage::new(
            Vec::new(),
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-reconciled"
                    .to_owned(),
            )),
        );
        let recovery_workflow = MailWorkflow::new(
            FixtureProvider::new(
                account.clone(),
                vec![cached_folder],
                [Ok(page_response(final_page))],
                Err(MailOperationError::ServiceUnavailable),
            ),
            reopened.clone(),
        );
        recovery_workflow
            .synchronize_account(
                &account,
                started_at + Duration::from_secs(1),
                &ignore_sync_progress,
            )
            .await
            .expect("a complete delta round reconciles the unknown update");
        assert!(
            !reopened
                .folder_write_sync_required(&folder_reference)
                .await
                .expect("completed delta round clears the recovery marker")
        );
        assert_eq!(
            reopened
                .read_message(&folder_reference, &message_id())
                .await
                .expect("message remains readable after reconciliation")
                .and_then(|(message, _)| message.metadata().follow_up_flag_status()),
            Some(FollowUpFlagStatus::NotFlagged)
        );

        drop(recovery_workflow);
        reopened.close_for_tests().await;
    });
}

#[test]
fn failure_to_commit_flag_recovery_marker_prevents_provider_dispatch() {
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
        let updates = Arc::clone(&provider.follow_up_flag_updates);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(provider, cache.clone());
        workflow
            .synchronize_account(&account, started_at, &ignore_sync_progress)
            .await
            .expect("synthetic message is cached before the flag action");
        cache
            .execute_batch_for_tests(
                "CREATE TRIGGER fail_flag_marker_insert
                 BEFORE INSERT ON mail_folder_write_sync_requirement
                 BEGIN SELECT RAISE(ABORT, 'synthetic marker failure'); END;",
            )
            .await
            .expect("synthetic failure trigger is installed");

        assert_eq!(
            workflow
                .set_message_follow_up_flag_status(
                    &folder_reference,
                    &message_id(),
                    FollowUpFlagStatus::Flagged,
                )
                .await,
            Err(MailWorkflowError::Cache(MailCacheError::Unavailable))
        );
        assert!(
            updates
                .lock()
                .expect("synthetic flag update log mutex is healthy")
                .is_empty()
        );
        assert!(
            !cache
                .folder_write_sync_required(&folder_reference)
                .await
                .expect("failed marker transaction leaves no recovery marker")
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
fn confirmed_message_move_reconciles_duplicate_cache_rows_and_preserves_body() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account);
        let archive = archive_folder(&account);
        let inbox_reference = inbox.reference().clone();
        let archive_reference = archive.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let cutoff = started_at - Duration::from_secs(90 * 24 * 60 * 60);
        let source_page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Source subject".to_owned(), cutoff),
            }],
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-inbox".to_owned(),
            )),
        );
        let mut destination_patch = complete_patch("Destination subject".to_owned(), cutoff);
        destination_patch.set_subject(None);
        let destination_page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: destination_patch,
            }],
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-archive"
                    .to_owned(),
            )),
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![inbox, archive],
            [
                Ok(page_response(source_page)),
                Ok(page_response(destination_page)),
            ],
            Err(MailOperationError::ServiceUnavailable),
        );
        let move_calls = Arc::clone(&provider.message_moves);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(provider, cache.clone());
        workflow
            .synchronize_account(&account, started_at, &ignore_sync_progress)
            .await
            .expect("duplicate message rows are cached in separate folders");
        cache
            .store_message_body(
                &inbox_reference,
                &message_id(),
                MessageBody::new(MessageBodyType::Text, "Synthetic cached body".to_owned()),
            )
            .await
            .expect("source body is stored before the move");

        assert_eq!(
            workflow
                .move_message(&inbox_reference, &message_id(), &archive_reference)
                .await,
            Ok(einfach_mail_application::MessageMoveOutcome::ConfirmedAndCached)
        );
        assert_eq!(
            move_calls
                .lock()
                .expect("synthetic move log mutex is not poisoned")
                .len(),
            1
        );
        assert_eq!(
            cache
                .read_message(&inbox_reference, &message_id())
                .await
                .expect("source lookup succeeds"),
            None
        );
        let archived = cache
            .read_message(&archive_reference, &message_id())
            .await
            .expect("destination lookup succeeds")
            .expect("message remains in the destination folder");
        assert_eq!(archived.0.folder(), &archive_reference);
        assert_eq!(archived.0.metadata().subject(), Some("Source subject"));
        assert_eq!(
            archived
                .1
                .expect("source body is copied when destination body is missing")
                .content(),
            "Synthetic cached body"
        );
        drop(workflow);
        cache.close_for_tests().await;

        let reopened = open_cache(&directory, account).await;
        let archived_after_restart = reopened
            .read_message(&archive_reference, &message_id())
            .await
            .expect("archive row is readable after reopening")
            .expect("moved message persists in the archive");
        assert_eq!(
            archived_after_restart
                .1
                .expect("moved body persists after reopening")
                .content(),
            "Synthetic cached body"
        );
        reopened.close_for_tests().await;
    });
}

#[test]
fn message_move_rejects_a_destination_outside_the_visible_folder_tree() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account);
        let archive = archive_folder(&account);
        let inbox_reference = inbox.reference().clone();
        let hidden_reference = FolderReference::new(
            account.clone(),
            FolderId::try_from("hidden".to_owned()).expect("synthetic folder ID is valid"),
        );
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let cutoff = started_at - Duration::from_secs(90 * 24 * 60 * 60);
        let source_page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Synthetic subject".to_owned(), cutoff),
            }],
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-inbox".to_owned(),
            )),
        );
        let empty_archive_page = MessageDeltaPage::new(
            Vec::new(),
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-archive"
                    .to_owned(),
            )),
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![inbox, archive],
            [
                Ok(page_response(source_page)),
                Ok(page_response(empty_archive_page)),
            ],
            Err(MailOperationError::ServiceUnavailable),
        );
        let move_calls = Arc::clone(&provider.message_moves);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(provider, cache.clone());
        workflow
            .synchronize_account(&account, started_at, &ignore_sync_progress)
            .await
            .expect("synthetic source message is cached");

        assert_eq!(
            workflow
                .move_message(&inbox_reference, &message_id(), &hidden_reference)
                .await,
            Err(MailWorkflowError::InvalidMoveDestination)
        );
        assert!(
            move_calls
                .lock()
                .expect("synthetic move log mutex is not poisoned")
                .is_empty()
        );
        cache.close_for_tests().await;
    });
}

#[test]
fn delete_message_requires_a_current_sync_before_resolving_deleted_items() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let provider = FixtureProvider::new(
            account.clone(),
            Vec::new(),
            [],
            Err(MailOperationError::ServiceUnavailable),
        );
        let folder_calls = Arc::clone(&provider.well_known_folder_calls);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(provider, cache.clone());
        let source = folder(&account).reference().clone();

        assert_eq!(
            workflow
                .move_message_to_well_known_folder(
                    &source,
                    &message_id(),
                    WellKnownMailFolder::DeletedItems
                )
                .await,
            Err(MailWorkflowError::AccountSyncRequired)
        );
        assert_eq!(folder_calls.load(Ordering::Relaxed), 0);
        cache.close_for_tests().await;
    });
}

#[test]
fn delete_message_uses_visible_deleted_items_id_and_preserves_cached_body() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account);
        let deleted = deleted_items_folder(&account);
        let inbox_reference = inbox.reference().clone();
        let deleted_reference = deleted.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let source_page = completed_message_delta_page(
            "delete-source",
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Synthetic subject".to_owned(), started_at),
            }],
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![inbox, deleted],
            [
                Ok(page_response(source_page)),
                Ok(page_response(completed_message_delta_page(
                    "delete-destination",
                    Vec::new(),
                ))),
            ],
            Err(MailOperationError::ServiceUnavailable),
        );
        let folder_calls = Arc::clone(&provider.well_known_folder_calls);
        let move_calls = Arc::clone(&provider.message_moves);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(provider, cache.clone());
        workflow
            .synchronize_account(&account, started_at, &ignore_sync_progress)
            .await
            .expect("synthetic inbox and Deleted Items sync");
        cache
            .store_message_body(
                &inbox_reference,
                &message_id(),
                MessageBody::new(MessageBodyType::Text, "Synthetic cached body".to_owned()),
            )
            .await
            .expect("synthetic message body is cached before deletion");

        assert_eq!(
            workflow
                .move_message_to_well_known_folder(
                    &inbox_reference,
                    &message_id(),
                    WellKnownMailFolder::DeletedItems,
                )
                .await
                .map(|outcome| outcome.cache_outcome()),
            Ok(einfach_mail_application::MessageMoveOutcome::ConfirmedAndCached)
        );
        assert_eq!(folder_calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            move_calls
                .lock()
                .expect("synthetic move log mutex is not poisoned")
                .as_slice(),
            &[(
                inbox_reference.clone(),
                message_id(),
                deleted_reference.clone()
            )]
        );
        assert_eq!(
            cache
                .read_message(&inbox_reference, &message_id())
                .await
                .expect("source lookup succeeds after deletion"),
            None
        );
        let deleted_message = cache
            .read_message(&deleted_reference, &message_id())
            .await
            .expect("Deleted Items lookup succeeds")
            .expect("message is cached in Deleted Items");
        assert_eq!(
            deleted_message
                .1
                .expect("encrypted body moves with its message")
                .content(),
            "Synthetic cached body"
        );
        drop(workflow);
        cache.close_for_tests().await;

        let reopened = open_cache(&directory, account).await;
        let persisted = reopened
            .read_message(&deleted_reference, &message_id())
            .await
            .expect("Deleted Items message remains readable after reopening")
            .expect("deleted message persists in the encrypted cache");
        assert_eq!(
            persisted
                .1
                .expect("encrypted body persists after reopening")
                .content(),
            "Synthetic cached body"
        );
        reopened.close_for_tests().await;
    });
}

#[test]
fn archive_action_resolves_visible_archive_and_preserves_cached_body() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account);
        let archive = archive_folder(&account);
        let inbox_reference = inbox.reference().clone();
        let archive_reference = archive.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let source_page = completed_message_delta_page(
            "archive-action-source",
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Synthetic subject".to_owned(), started_at),
            }],
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![inbox, archive],
            [
                Ok(page_response(source_page)),
                Ok(page_response(completed_message_delta_page(
                    "archive-action-destination",
                    Vec::new(),
                ))),
            ],
            Err(MailOperationError::ServiceUnavailable),
        );
        let folder_calls = Arc::clone(&provider.well_known_folder_calls);
        let move_calls = Arc::clone(&provider.message_moves);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(provider, cache.clone());
        workflow
            .synchronize_account(&account, started_at, &ignore_sync_progress)
            .await
            .expect("synthetic inbox and Archive sync");
        cache
            .store_message_body(
                &inbox_reference,
                &message_id(),
                MessageBody::new(MessageBodyType::Text, "Synthetic archived body".to_owned()),
            )
            .await
            .expect("synthetic message body is cached before archiving");

        let outcome = workflow
            .move_message_to_well_known_folder(
                &inbox_reference,
                &message_id(),
                WellKnownMailFolder::Archive,
            )
            .await
            .expect("Archive action resolves and confirms the visible well-known folder");

        assert_eq!(outcome.destination_folder(), &archive_reference);
        assert_eq!(
            outcome.cache_outcome(),
            einfach_mail_application::MessageMoveOutcome::ConfirmedAndCached
        );
        assert_eq!(folder_calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            move_calls
                .lock()
                .expect("synthetic move log mutex is not poisoned")
                .as_slice(),
            &[(
                inbox_reference.clone(),
                message_id(),
                archive_reference.clone(),
            )]
        );
        assert!(
            cache
                .read_message(&inbox_reference, &message_id())
                .await
                .expect("source cache lookup succeeds after archiving")
                .is_none()
        );
        let archived_message = cache
            .read_message(&archive_reference, &message_id())
            .await
            .expect("Archive cache lookup succeeds")
            .expect("message is cached in Archive");
        assert_eq!(
            archived_message
                .1
                .expect("encrypted body moves with the message")
                .content(),
            "Synthetic archived body"
        );
        cache.close_for_tests().await;
    });
}

#[test]
fn archive_action_rejects_a_message_already_in_archive() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let archive = archive_folder(&account);
        let archive_reference = archive.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let page = completed_message_delta_page(
            "already-in-archive",
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Synthetic subject".to_owned(), started_at),
            }],
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![archive],
            [Ok(page_response(page))],
            Err(MailOperationError::ServiceUnavailable),
        );
        let folder_calls = Arc::clone(&provider.well_known_folder_calls);
        let move_calls = Arc::clone(&provider.message_moves);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(provider, cache.clone());
        workflow
            .synchronize_account(&account, started_at, &ignore_sync_progress)
            .await
            .expect("synthetic Archive sync completes");

        assert_eq!(
            workflow
                .move_message_to_well_known_folder(
                    &archive_reference,
                    &message_id(),
                    WellKnownMailFolder::Archive,
                )
                .await,
            Err(MailWorkflowError::AlreadyInWellKnownFolder(
                WellKnownMailFolder::Archive
            ))
        );
        assert_eq!(folder_calls.load(Ordering::Relaxed), 1);
        assert!(
            move_calls
                .lock()
                .expect("synthetic move log mutex is not poisoned")
                .is_empty()
        );
        cache.close_for_tests().await;
    });
}

#[test]
fn well_known_folder_rejects_a_resolved_folder_outside_the_visible_tree() {
    smol::block_on(async {
        for destination in [
            WellKnownMailFolder::Archive,
            WellKnownMailFolder::DeletedItems,
        ] {
            let directory = TempDir::new().expect("temporary directory is created");
            let account = account_id();
            let inbox = folder(&account);
            let inbox_reference = inbox.reference().clone();
            let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
            let source_page = completed_message_delta_page(
                "invisible-special-folder-source",
                vec![MessageDeltaChange::Upsert {
                    id: message_id(),
                    patch: complete_patch("Synthetic subject".to_owned(), started_at),
                }],
            );
            let mut provider = FixtureProvider::new(
                account.clone(),
                vec![
                    inbox,
                    archive_folder(&account),
                    deleted_items_folder(&account),
                ],
                [
                    Ok(page_response(source_page)),
                    Ok(page_response(completed_message_delta_page(
                        "invisible-special-folder-archive",
                        Vec::new(),
                    ))),
                    Ok(page_response(completed_message_delta_page(
                        "invisible-special-folder-deleted",
                        Vec::new(),
                    ))),
                ],
                Err(MailOperationError::ServiceUnavailable),
            );
            let not_visible =
                FolderId::try_from("not-visible".to_owned()).expect("synthetic folder ID is valid");
            match destination {
                WellKnownMailFolder::Archive => provider.archive_folder_id = not_visible,
                WellKnownMailFolder::DeletedItems => provider.deleted_items_folder_id = not_visible,
            }
            let folder_calls = Arc::clone(&provider.well_known_folder_calls);
            let move_calls = Arc::clone(&provider.message_moves);
            let cache = open_cache(&directory, account.clone()).await;
            let workflow = MailWorkflow::new(provider, cache.clone());
            workflow
                .synchronize_account(&account, started_at, &ignore_sync_progress)
                .await
                .expect("synthetic account sync completes");

            assert_eq!(
                workflow
                    .move_message_to_well_known_folder(
                        &inbox_reference,
                        &message_id(),
                        destination,
                    )
                    .await,
                Err(MailWorkflowError::WellKnownFolderUnavailable(destination))
            );
            assert_eq!(folder_calls.load(Ordering::Relaxed), 1);
            assert!(
                move_calls
                    .lock()
                    .expect("synthetic move log mutex is not poisoned")
                    .is_empty()
            );
            assert!(
                cache
                    .read_message(&inbox_reference, &message_id())
                    .await
                    .expect("source cache lookup succeeds")
                    .is_some()
            );
            cache.close_for_tests().await;
        }
    });
}

#[test]
fn delete_message_rejects_a_message_already_in_deleted_items() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let deleted = deleted_items_folder(&account);
        let deleted_reference = deleted.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let page = completed_message_delta_page(
            "already-deleted",
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Synthetic subject".to_owned(), started_at),
            }],
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![deleted],
            [Ok(page_response(page))],
            Err(MailOperationError::ServiceUnavailable),
        );
        let folder_calls = Arc::clone(&provider.well_known_folder_calls);
        let move_calls = Arc::clone(&provider.message_moves);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(provider, cache.clone());
        workflow
            .synchronize_account(&account, started_at, &ignore_sync_progress)
            .await
            .expect("synthetic Deleted Items sync completes");

        assert_eq!(
            workflow
                .move_message_to_well_known_folder(
                    &deleted_reference,
                    &message_id(),
                    WellKnownMailFolder::DeletedItems,
                )
                .await,
            Err(MailWorkflowError::AlreadyInWellKnownFolder(
                WellKnownMailFolder::DeletedItems
            ))
        );
        assert_eq!(folder_calls.load(Ordering::Relaxed), 1);
        assert!(
            move_calls
                .lock()
                .expect("synthetic move log mutex is not poisoned")
                .is_empty()
        );
        assert!(
            cache
                .read_message(&deleted_reference, &message_id())
                .await
                .expect("Deleted Items cache lookup succeeds")
                .is_some()
        );
        cache.close_for_tests().await;
    });
}

#[test]
fn unknown_delete_outcome_requires_sync_before_another_move_request() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account);
        let deleted = deleted_items_folder(&account);
        let inbox_reference = inbox.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let source_page = || {
            completed_message_delta_page(
                "unknown-delete-source",
                vec![MessageDeltaChange::Upsert {
                    id: message_id(),
                    patch: complete_patch("Synthetic subject".to_owned(), started_at),
                }],
            )
        };
        let provider = FixtureProvider::new(
            account.clone(),
            vec![inbox, deleted],
            [
                Ok(page_response(source_page())),
                Ok(page_response(completed_message_delta_page(
                    "unknown-delete-destination",
                    Vec::new(),
                ))),
                Ok(page_response(source_page())),
                Ok(page_response(completed_message_delta_page(
                    "unknown-delete-resync-destination",
                    Vec::new(),
                ))),
            ],
            Err(MailOperationError::ServiceUnavailable),
        );
        *provider
            .move_result
            .lock()
            .expect("synthetic move result mutex is not poisoned") =
            Err(MailOperationError::MoveOutcomeUnknown);
        let move_calls = Arc::clone(&provider.message_moves);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(provider.clone(), cache.clone());
        workflow
            .synchronize_account(&account, started_at, &ignore_sync_progress)
            .await
            .expect("synthetic account sync completes");

        assert_eq!(
            workflow
                .move_message_to_well_known_folder(
                    &inbox_reference,
                    &message_id(),
                    WellKnownMailFolder::DeletedItems
                )
                .await,
            Err(MailWorkflowError::Provider(
                MailOperationError::MoveOutcomeUnknown
            ))
        );
        assert!(
            cache
                .read_message(&inbox_reference, &message_id())
                .await
                .expect("source cache lookup succeeds after unknown outcome")
                .is_some()
        );
        assert_eq!(
            workflow
                .move_message_to_well_known_folder(
                    &inbox_reference,
                    &message_id(),
                    WellKnownMailFolder::DeletedItems
                )
                .await,
            Err(MailWorkflowError::AccountSyncRequired)
        );
        assert_eq!(
            move_calls
                .lock()
                .expect("synthetic move log mutex is not poisoned")
                .len(),
            1
        );

        workflow
            .synchronize_account(
                &account,
                started_at + Duration::from_secs(1),
                &ignore_sync_progress,
            )
            .await
            .expect("a new complete account sync restores message writes");
        *provider
            .move_result
            .lock()
            .expect("synthetic move result mutex is not poisoned") = Ok(());
        assert_eq!(
            workflow
                .move_message_to_well_known_folder(
                    &inbox_reference,
                    &message_id(),
                    WellKnownMailFolder::DeletedItems,
                )
                .await
                .map(|outcome| outcome.cache_outcome()),
            Ok(einfach_mail_application::MessageMoveOutcome::ConfirmedAndCached)
        );
        assert_eq!(
            move_calls
                .lock()
                .expect("synthetic move log mutex is not poisoned")
                .len(),
            2
        );
        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn canceled_delete_request_requires_sync_before_another_move_request() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account);
        let deleted = deleted_items_folder(&account);
        let inbox_reference = inbox.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let source_page = completed_message_delta_page(
            "canceled-delete-source",
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Synthetic subject".to_owned(), started_at),
            }],
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![inbox, deleted],
            [
                Ok(page_response(source_page)),
                Ok(page_response(completed_message_delta_page(
                    "canceled-delete-destination",
                    Vec::new(),
                ))),
            ],
            Err(MailOperationError::ServiceUnavailable),
        );
        let (move_started_receiver, _release_move) = provider.hold_next_message_move();
        let move_calls = Arc::clone(&provider.message_moves);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(provider, cache.clone());
        workflow
            .synchronize_account(&account, started_at, &ignore_sync_progress)
            .await
            .expect("synthetic account sync completes");

        smol::future::race(
            async {
                let _ = workflow
                    .move_message_to_well_known_folder(
                        &inbox_reference,
                        &message_id(),
                        WellKnownMailFolder::DeletedItems,
                    )
                    .await;
            },
            async {
                move_started_receiver
                    .recv()
                    .await
                    .expect("provider move reaches its cancellation gate");
            },
        )
        .await;

        assert_eq!(
            workflow
                .move_message_to_well_known_folder(
                    &inbox_reference,
                    &message_id(),
                    WellKnownMailFolder::DeletedItems
                )
                .await,
            Err(MailWorkflowError::AccountSyncRequired)
        );
        assert_eq!(
            move_calls
                .lock()
                .expect("synthetic move log mutex is not poisoned")
                .len(),
            1
        );
        assert!(
            cache
                .read_message(&inbox_reference, &message_id())
                .await
                .expect("source cache lookup succeeds after cancellation")
                .is_some()
        );
        cache.close_for_tests().await;
    });
}

#[test]
fn delete_message_keeps_the_source_reservation_while_resolving_deleted_items() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account);
        let archive = archive_folder(&account);
        let deleted = deleted_items_folder(&account);
        let inbox_reference = inbox.reference().clone();
        let archive_reference = archive.reference().clone();
        let deleted_reference = deleted.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let provider = FixtureProvider::new(
            account.clone(),
            vec![inbox, archive, deleted],
            [
                Ok(page_response(completed_message_delta_page(
                    "reserved-delete-source",
                    vec![MessageDeltaChange::Upsert {
                        id: message_id(),
                        patch: complete_patch("Synthetic subject".to_owned(), started_at),
                    }],
                ))),
                Ok(page_response(completed_message_delta_page(
                    "reserved-delete-archive",
                    Vec::new(),
                ))),
                Ok(page_response(completed_message_delta_page(
                    "reserved-delete-destination",
                    Vec::new(),
                ))),
            ],
            Err(MailOperationError::ServiceUnavailable),
        );
        let (lookup_started, release_lookup) = provider.hold_next_well_known_folder_lookup();
        let move_calls = Arc::clone(&provider.message_moves);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = Arc::new(MailWorkflow::new(provider, cache.clone()));
        workflow
            .synchronize_account(&account, started_at, &ignore_sync_progress)
            .await
            .expect("synthetic account sync completes");
        let executor = smol::Executor::new();
        let delete_workflow = Arc::clone(&workflow);
        let delete_source = inbox_reference.clone();

        let (delete_result, competing_move) = smol::block_on(executor.run(async {
            let delete_task = executor.spawn(async move {
                delete_workflow
                    .move_message_to_well_known_folder(
                        &delete_source,
                        &message_id(),
                        WellKnownMailFolder::DeletedItems,
                    )
                    .await
            });
            lookup_started
                .recv()
                .await
                .expect("Deleted Items resolver blocks after source reservation");
            let competing_move = workflow
                .move_message(&inbox_reference, &message_id(), &archive_reference)
                .await;
            release_lookup
                .send(())
                .await
                .expect("Deleted Items resolver gate remains open");
            (delete_task.await, competing_move)
        }));

        assert_eq!(competing_move, Err(MailWorkflowError::OperationInProgress));
        assert_eq!(
            delete_result.map(|outcome| {
                (
                    outcome.destination_folder().clone(),
                    outcome.cache_outcome(),
                )
            }),
            Ok((
                deleted_reference,
                einfach_mail_application::MessageMoveOutcome::ConfirmedAndCached,
            ))
        );
        assert_eq!(
            move_calls
                .lock()
                .expect("synthetic move log mutex is not poisoned")
                .len(),
            1
        );
        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn confirmed_delete_with_cache_failure_requires_sync_before_retry() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account);
        let deleted = deleted_items_folder(&account);
        let inbox_reference = inbox.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let source_page = completed_message_delta_page(
            "failed-cache-delete-source",
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Synthetic subject".to_owned(), started_at),
            }],
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![inbox, deleted],
            [
                Ok(page_response(source_page)),
                Ok(page_response(completed_message_delta_page(
                    "failed-cache-delete-destination",
                    Vec::new(),
                ))),
            ],
            Err(MailOperationError::ServiceUnavailable),
        );
        let folder_calls = Arc::clone(&provider.well_known_folder_calls);
        let move_calls = Arc::clone(&provider.message_moves);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(provider, cache.clone());
        workflow
            .synchronize_account(&account, started_at, &ignore_sync_progress)
            .await
            .expect("synthetic account sync completes");
        cache.set_message_move_failure_for_tests(true).await;

        assert_eq!(
            workflow
                .move_message_to_well_known_folder(
                    &inbox_reference,
                    &message_id(),
                    WellKnownMailFolder::DeletedItems,
                )
                .await
                .map(|outcome| outcome.cache_outcome()),
            Ok(einfach_mail_application::MessageMoveOutcome::ConfirmedNeedsSync)
        );
        assert_eq!(
            workflow
                .move_message_to_well_known_folder(
                    &inbox_reference,
                    &message_id(),
                    WellKnownMailFolder::DeletedItems
                )
                .await,
            Err(MailWorkflowError::AccountSyncRequired)
        );
        assert_eq!(folder_calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            move_calls
                .lock()
                .expect("synthetic move log mutex is not poisoned")
                .len(),
            1
        );
        assert!(
            cache
                .read_message(&inbox_reference, &message_id())
                .await
                .expect("source cache lookup succeeds after transaction rollback")
                .is_some()
        );
        cache.close_for_tests().await;
    });
}

#[test]
fn message_move_cache_failure_rolls_back_and_requests_sync() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account);
        let archive = archive_folder(&account);
        let inbox_reference = inbox.reference().clone();
        let archive_reference = archive.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let cutoff = started_at - Duration::from_secs(90 * 24 * 60 * 60);
        let source_page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Synthetic subject".to_owned(), cutoff),
            }],
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-inbox".to_owned(),
            )),
        );
        let empty_destination_page = MessageDeltaPage::new(
            Vec::new(),
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-archive"
                    .to_owned(),
            )),
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![inbox, archive],
            [
                Ok(page_response(source_page)),
                Ok(page_response(empty_destination_page)),
            ],
            Err(MailOperationError::ServiceUnavailable),
        );
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(provider, cache.clone());
        workflow
            .synchronize_account(&account, started_at, &ignore_sync_progress)
            .await
            .expect("synthetic source message is cached");
        cache.set_message_move_failure_for_tests(true).await;

        assert_eq!(
            workflow
                .move_message(&inbox_reference, &message_id(), &archive_reference)
                .await,
            Ok(einfach_mail_application::MessageMoveOutcome::ConfirmedNeedsSync)
        );
        assert!(
            cache
                .read_message(&inbox_reference, &message_id())
                .await
                .expect("source lookup succeeds after rollback")
                .is_some()
        );
        assert!(
            cache
                .read_message(&archive_reference, &message_id())
                .await
                .expect("destination lookup succeeds after rollback")
                .is_none()
        );
        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn unknown_message_move_does_not_change_the_cached_folder() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account);
        let archive = archive_folder(&account);
        let inbox_reference = inbox.reference().clone();
        let archive_reference = archive.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let cutoff = started_at - Duration::from_secs(90 * 24 * 60 * 60);
        let source_page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Synthetic subject".to_owned(), cutoff),
            }],
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-inbox".to_owned(),
            )),
        );
        let empty_destination_page = MessageDeltaPage::new(
            Vec::new(),
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-archive"
                    .to_owned(),
            )),
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![inbox, archive],
            [
                Ok(page_response(source_page)),
                Ok(page_response(empty_destination_page)),
            ],
            Err(MailOperationError::ServiceUnavailable),
        );
        *provider
            .move_result
            .lock()
            .expect("synthetic move result mutex is not poisoned") =
            Err(MailOperationError::MoveOutcomeUnknown);
        let move_calls = Arc::clone(&provider.message_moves);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(provider, cache.clone());
        workflow
            .synchronize_account(&account, started_at, &ignore_sync_progress)
            .await
            .expect("synthetic source message is cached");

        assert_eq!(
            workflow
                .move_message(&inbox_reference, &message_id(), &archive_reference)
                .await,
            Err(MailWorkflowError::Provider(
                MailOperationError::MoveOutcomeUnknown
            ))
        );
        assert!(
            cache
                .read_message(&inbox_reference, &message_id())
                .await
                .expect("source lookup succeeds")
                .is_some()
        );
        assert!(
            cache
                .read_message(&archive_reference, &message_id())
                .await
                .expect("destination lookup succeeds")
                .is_none()
        );
        assert_eq!(
            move_calls
                .lock()
                .expect("synthetic move log mutex is not poisoned")
                .len(),
            1
        );
        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn account_sync_reservation_blocks_moves_before_folder_tree_replacement() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account);
        let archive = archive_folder(&account);
        let inbox_reference = inbox.reference().clone();
        let archive_reference = archive.reference().clone();
        let empty_page = || {
            page_response(MessageDeltaPage::new(
                Vec::new(),
                MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                    "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-empty"
                        .to_owned(),
                )),
            ))
        };
        let mut provider = FixtureProvider::new(
            account.clone(),
            vec![inbox, archive],
            [Ok(empty_page()), Ok(empty_page())],
            Err(MailOperationError::ServiceUnavailable),
        );
        let (folder_list_started, folder_list_started_rx) = smol::channel::unbounded();
        let (release_folder_list_tx, release_folder_list) = smol::channel::unbounded();
        provider.folder_list_gate = Some((folder_list_started, release_folder_list));
        let move_calls = Arc::clone(&provider.message_moves);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = Arc::new(MailWorkflow::new(provider, cache.clone()));
        let sync_workflow = Arc::clone(&workflow);
        let sync_task = smol::spawn(async move {
            sync_workflow
                .synchronize_account(
                    &account,
                    UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                    &ignore_sync_progress,
                )
                .await
        });
        folder_list_started_rx
            .recv()
            .await
            .expect("account sync reaches folder listing");

        assert_eq!(
            workflow
                .move_message(&inbox_reference, &message_id(), &archive_reference)
                .await,
            Err(MailWorkflowError::OperationInProgress)
        );
        assert!(
            move_calls
                .lock()
                .expect("synthetic move log mutex is not poisoned")
                .is_empty()
        );

        release_folder_list_tx
            .send(())
            .await
            .expect("folder listing gate is still open");
        sync_task
            .await
            .expect("account synchronization owns the folder tree and its delta feeds");
        drop(workflow);
        cache.close_for_tests().await;
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
        removal_cache.close_for_tests().await;
        drop(removal_cache);
        std::fs::remove_file(directory.path().join("mail.sqlite3"))
            .expect("cache replacement unlinks the database before the stale handle stages a send");

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
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            workflow.load_pending_send(&account).await,
            Ok(Some(pending.clone()))
        );
        drop(workflow);
        cache.close_for_tests().await;
        drop(cache);

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
        assert_eq!(
            <SqliteMailCache as MailCache>::remove_account(&reopened, &account).await,
            Err(MailCacheError::SendAlreadyPending)
        );
        assert_eq!(
            <SqliteMailCache as einfach_mail_application::CalendarCache>::remove_account(
                &reopened, &account,
            )
            .await,
            Err(CalendarCacheError::PendingMailSend)
        );

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
fn unknown_reply_marker_survives_encrypted_restart_without_reply_body() {
    smol::block_on(async {
        let directory = TempDir::new().expect("synthetic cache directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let cutoff = started_at - Duration::from_secs(90 * 24 * 60 * 60);
        let delta_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-reply".to_owned(),
        );
        let page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Synthetic source subject".to_owned(), cutoff),
            }],
            MessageDeltaProgress::Complete(delta_link),
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            [Ok(page_response(page))],
            Err(MailOperationError::ServiceUnavailable),
        )
        .with_send_result(Err(MailSendProviderError::OutcomeUnknown));
        let send_calls = Arc::clone(&provider.send_calls);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(provider.clone(), cache.clone());
        workflow
            .synchronize_account(&account, started_at, &ignore_sync_progress)
            .await
            .expect("synthetic reply source is cached");
        let source = MailReplySource::new(folder_reference.clone(), message_id());
        let reply_body = "Synthetic private reply body";
        let result = workflow
            .reply_to_message(
                &account,
                MailReplyDraft::new(source.clone(), reply_body.to_owned()),
                started_at,
            )
            .await
            .expect("unknown reply result is retained for explicit resolution");
        let pending = match result {
            MailSendOutcome::OutcomeUnknown(pending) => pending,
            other => panic!("expected unresolved reply marker, got {other:?}"),
        };
        assert_eq!(pending.target().subject(), Some("Synthetic source subject"));
        assert_eq!(pending.target().reply_source(), Some(&source));
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            workflow
                .reply_to_message(
                    &account,
                    MailReplyDraft::new(source.clone(), reply_body.to_owned()),
                    started_at,
                )
                .await,
            Err(MailSendWorkflowError::PendingResolutionRequired(
                pending.clone()
            ))
        );
        assert_eq!(
            workflow
                .send_message(
                    &account,
                    draft("Another subject", "another body"),
                    started_at
                )
                .await,
            Err(MailSendWorkflowError::PendingResolutionRequired(
                pending.clone()
            ))
        );
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            cache.load_pending_send(&account).await,
            Ok(Some(pending.clone()))
        );
        assert_eq!(
            <SqliteMailCache as MailCache>::remove_account(&cache, &account).await,
            Err(MailCacheError::SendAlreadyPending)
        );
        assert_eq!(
            <SqliteMailCache as einfach_mail_application::CalendarCache>::remove_account(
                &cache, &account,
            )
            .await,
            Err(CalendarCacheError::PendingMailSend)
        );

        drop(workflow);
        cache.close_for_tests().await;
        let encrypted_bytes = std::fs::read(directory.path().join("mail.sqlite3"))
            .expect("encrypted cache remains on disk");
        assert!(
            !encrypted_bytes
                .windows(reply_body.len())
                .any(|window| window == reply_body.as_bytes())
        );

        let reopened = open_cache(&directory, account.clone()).await;
        assert_eq!(
            reopened.load_pending_send(&account).await,
            Ok(Some(pending.clone()))
        );
        let workflow = MailWorkflow::new(provider, reopened.clone());
        assert_eq!(
            workflow
                .reply_to_message(
                    &account,
                    MailReplyDraft::new(source, reply_body.to_owned()),
                    started_at,
                )
                .await,
            Err(MailSendWorkflowError::PendingResolutionRequired(
                pending.clone()
            ))
        );
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);
        workflow
            .resolve_pending_send(&account, pending.operation_id())
            .await
            .expect("explicit Sent Items reconciliation clears the marker");
        assert_eq!(workflow.load_pending_send(&account).await, Ok(None));
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);
        drop(workflow);
        reopened.close_for_tests().await;
    });
}

#[test]
fn reply_requires_a_cached_source_owned_by_the_requested_account() {
    smol::block_on(async {
        let directory = TempDir::new().expect("synthetic cache directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let delta_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-empty".to_owned(),
        );
        let empty_page =
            MessageDeltaPage::new(Vec::new(), MessageDeltaProgress::Complete(delta_link));
        let provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            [Ok(page_response(empty_page))],
            Err(MailOperationError::ServiceUnavailable),
        );
        let send_calls = Arc::clone(&provider.send_calls);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(provider, cache.clone());
        workflow
            .synchronize_account(&account, started_at, &ignore_sync_progress)
            .await
            .expect("the empty synthetic folder is cached");

        let other_account = AccountId::try_from("synthetic-other-account".to_owned())
            .expect("synthetic account ID is valid");
        let other_folder = folder(&other_account).reference().clone();
        assert_eq!(
            workflow
                .reply_to_message(
                    &account,
                    MailReplyDraft::new(
                        MailReplySource::new(other_folder, message_id()),
                        "Synthetic reply body".to_owned(),
                    ),
                    started_at,
                )
                .await,
            Err(MailSendWorkflowError::Provider(
                MailSendProviderError::AccountMismatch
            ))
        );
        assert_eq!(
            workflow
                .reply_to_message(
                    &account,
                    MailReplyDraft::new(
                        MailReplySource::new(folder_reference, message_id()),
                        "Synthetic reply body".to_owned(),
                    ),
                    started_at,
                )
                .await,
            Err(MailSendWorkflowError::ReplySourceNotCached)
        );
        assert_eq!(send_calls.load(Ordering::Relaxed), 0);
        assert_eq!(cache.load_pending_send(&account).await, Ok(None));
        drop(workflow);
        cache.close_for_tests().await;
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
            Ok(Some((*pending).clone()))
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

#[test]
fn account_removal_reservation_blocks_new_mail_work_before_provider_calls() {
    smol::block_on(async {
        let directory = TempDir::new().expect("synthetic account directory is created");
        let account = account_id();
        let inbox = folder(&account);
        let archive = archive_folder(&account);
        let inbox_reference = inbox.reference().clone();
        let archive_reference = archive.reference().clone();
        let mut provider = FixtureProvider::new(
            account.clone(),
            vec![inbox, archive],
            [],
            Err(MailOperationError::ServiceUnavailable),
        );
        provider.send_result = Ok(());
        let folder_list_calls = Arc::clone(&provider.folder_list_calls);
        let send_calls = Arc::clone(&provider.send_calls);
        let read_state_updates = Arc::clone(&provider.read_state_updates);
        let message_moves = Arc::clone(&provider.message_moves);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = Arc::new(MailWorkflow::new(provider, cache.clone()));
        let (marker_check_finished, resume_removal) =
            cache.hold_removal_after_marker_check_for_tests();
        let removing_workflow = Arc::clone(&workflow);
        let removing_account = account.clone();
        let removal_task = smol::spawn(async move {
            removing_workflow
                .remove_account_cache(&removing_account)
                .await
        });
        marker_check_finished
            .recv()
            .await
            .expect("removal checked the empty pending-send marker");

        assert_eq!(
            workflow
                .synchronize_account(
                    &account,
                    UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                    &ignore_sync_progress,
                )
                .await,
            Err(MailWorkflowError::OperationInProgress)
        );
        assert_eq!(
            workflow
                .set_message_read_state(&inbox_reference, &message_id(), ReadState::Read)
                .await,
            Err(MailWorkflowError::OperationInProgress)
        );
        assert_eq!(
            workflow
                .move_message(&inbox_reference, &message_id(), &archive_reference)
                .await,
            Err(MailWorkflowError::OperationInProgress)
        );
        assert_eq!(
            workflow
                .send_message(
                    &account,
                    draft("Synthetic subject", "Synthetic body"),
                    UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                )
                .await,
            Err(MailSendWorkflowError::OperationInProgress)
        );
        assert_eq!(folder_list_calls.load(Ordering::Relaxed), 0);
        assert_eq!(send_calls.load(Ordering::Relaxed), 0);
        assert!(
            read_state_updates
                .lock()
                .expect("synthetic read-state log mutex is healthy")
                .is_empty()
        );
        assert!(
            message_moves
                .lock()
                .expect("synthetic message-move log mutex is healthy")
                .is_empty()
        );

        resume_removal
            .send(())
            .await
            .expect("account removal gate is released");
        let removal_result = removal_task.await;
        assert_removal_completed(removal_result, &directory.path().join("mail.sqlite3"));
    });
}

#[test]
fn active_send_reservation_blocks_account_removal_before_marker_staging() {
    smol::block_on(async {
        let directory = TempDir::new().expect("synthetic account directory is created");
        let account = account_id();
        let mut provider = FixtureProvider::new(
            account.clone(),
            Vec::new(),
            [],
            Err(MailOperationError::ServiceUnavailable),
        );
        provider.send_result = Ok(());
        let send_calls = Arc::clone(&provider.send_calls);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = Arc::new(MailWorkflow::new(provider, cache.clone()));
        let (marker_read_started, release_marker_read) = cache.hold_pending_send_read_for_tests();
        let sending_workflow = Arc::clone(&workflow);
        let sending_account = account.clone();
        let send_task = smol::spawn(async move {
            sending_workflow
                .send_message(
                    &sending_account,
                    draft("Synthetic subject", "Synthetic body"),
                    UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                )
                .await
        });
        marker_read_started
            .recv()
            .await
            .expect("send reserved the account before reading the marker");

        assert_eq!(cache.load_pending_send(&account).await, Ok(None));
        assert_eq!(
            workflow.remove_account_cache(&account).await,
            Err(MailCacheError::OperationInProgress)
        );
        assert_eq!(send_calls.load(Ordering::Relaxed), 0);
        release_marker_read
            .send(())
            .await
            .expect("synthetic send marker read is released");
        assert_eq!(send_task.await, Ok(MailSendOutcome::Accepted));
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);
        assert_eq!(workflow.load_pending_send(&account).await, Ok(None));
        let removal_result = workflow.remove_account_cache(&account).await;
        assert_removal_completed(removal_result, &directory.path().join("mail.sqlite3"));
    });
}

#[test]
fn reply_reservation_blocks_account_removal_before_and_during_submission() {
    smol::block_on(async {
        let directory = TempDir::new().expect("synthetic account directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut provider = provider_with_cached_message(
            account.clone(),
            vec![cached_folder],
            started_at,
            Err(MailOperationError::ServiceUnavailable),
        );
        provider.send_result = Err(MailSendProviderError::OutcomeUnknown);
        let send_calls = Arc::clone(&provider.send_calls);
        let (submission_started, release_submission) = provider.hold_next_submission();
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = Arc::new(MailWorkflow::new(provider, cache.clone()));
        workflow
            .synchronize_account(&account, started_at, &ignore_sync_progress)
            .await
            .expect("synthetic reply source is cached");

        let (marker_read_started, release_marker_read) = cache.hold_pending_send_read_for_tests();
        let replying_workflow = Arc::clone(&workflow);
        let replying_account = account.clone();
        let reply_source = MailReplySource::new(folder_reference, message_id());
        let reply_task = smol::spawn(async move {
            replying_workflow
                .reply_to_message(
                    &replying_account,
                    MailReplyDraft::new(reply_source, "Synthetic reply body".to_owned()),
                    started_at,
                )
                .await
        });
        marker_read_started
            .recv()
            .await
            .expect("reply reserved the account before reading the marker");

        assert_eq!(cache.load_pending_send(&account).await, Ok(None));
        assert_eq!(
            workflow.remove_account_cache(&account).await,
            Err(MailCacheError::OperationInProgress)
        );
        release_marker_read
            .send(())
            .await
            .expect("synthetic reply marker read is released");
        submission_started
            .recv()
            .await
            .expect("reply reaches the provider with its account reservation held");
        assert_eq!(
            workflow.remove_account_cache(&account).await,
            Err(MailCacheError::OperationInProgress)
        );
        release_submission
            .send(())
            .await
            .expect("synthetic reply submission is released");
        assert!(matches!(
            reply_task.await,
            Ok(MailSendOutcome::OutcomeUnknown(_))
        ));
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            workflow.remove_account_cache(&account).await,
            Err(MailCacheError::SendAlreadyPending)
        );
        assert!(
            cache
                .load_pending_send(&account)
                .await
                .expect("unresolved reply marker remains readable")
                .is_some()
        );
        assert!(directory.path().join("mail.sqlite3").exists());
        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn active_account_sync_blocks_cache_removal() {
    smol::block_on(async {
        let directory = TempDir::new().expect("synthetic account directory is created");
        let account = account_id();
        let mut provider = FixtureProvider::new(
            account.clone(),
            Vec::new(),
            [],
            Err(MailOperationError::ServiceUnavailable),
        );
        let (sync_started, sync_started_receiver) = smol::channel::bounded(1);
        let (release_sync, release_sync_receiver) = smol::channel::bounded(1);
        provider.folder_list_gate = Some((sync_started, release_sync_receiver));
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = Arc::new(MailWorkflow::new(provider, cache.clone()));
        let syncing_workflow = Arc::clone(&workflow);
        let syncing_account = account.clone();
        let sync_task = smol::spawn(async move {
            syncing_workflow
                .synchronize_account(
                    &syncing_account,
                    UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                    &ignore_sync_progress,
                )
                .await
        });
        sync_started_receiver
            .recv()
            .await
            .expect("account sync owns its reservation while listing folders");

        assert_eq!(
            workflow.remove_account_cache(&account).await,
            Err(MailCacheError::OperationInProgress)
        );
        assert!(directory.path().join("mail.sqlite3").exists());
        release_sync
            .send(())
            .await
            .expect("synthetic folder listing gate is released");
        assert!(sync_task.await.is_ok());
        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn active_folder_write_blocks_cache_removal() {
    smol::block_on(async {
        let directory = TempDir::new().expect("synthetic account directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let provider = provider_with_cached_message(
            account.clone(),
            vec![cached_folder],
            started_at,
            Err(MailOperationError::ServiceUnavailable),
        );
        let (write_started, release_write) = provider.hold_next_read_state();
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = Arc::new(MailWorkflow::new(provider, cache.clone()));
        workflow
            .synchronize_account(&account, started_at, &ignore_sync_progress)
            .await
            .expect("synthetic message is cached before its update");
        let writing_workflow = Arc::clone(&workflow);
        let writing_folder = folder_reference;
        let write_task = smol::spawn(async move {
            writing_workflow
                .set_message_read_state(&writing_folder, &message_id(), ReadState::Read)
                .await
        });
        write_started
            .recv()
            .await
            .expect("folder write owns its reservation while waiting for Graph");

        assert_eq!(
            workflow.remove_account_cache(&account).await,
            Err(MailCacheError::OperationInProgress)
        );
        release_write
            .send(())
            .await
            .expect("synthetic folder write is released");
        assert!(write_task.await.is_ok());
        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn uncached_message_body_fetch_blocks_cache_removal() {
    smol::block_on(async {
        let directory = TempDir::new().expect("synthetic account directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let body = MessageBody::new(MessageBodyType::Text, "Synthetic body".to_owned());
        let mut provider = provider_with_cached_message(
            account.clone(),
            vec![cached_folder],
            started_at,
            Ok(body.clone()),
        );
        let body_fetches = Arc::clone(&provider.body_fetches);
        let (fetch_started_sender, fetch_started) = smol::channel::bounded(1);
        let (release_fetch, fetch_release_receiver) = smol::channel::bounded(1);
        provider.body_gate = Some(BodyFetchGate {
            started: fetch_started_sender,
            release: fetch_release_receiver,
        });
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = Arc::new(MailWorkflow::new(provider, cache.clone()));
        workflow
            .synchronize_account(&account, started_at, &ignore_sync_progress)
            .await
            .expect("synthetic message is cached before opening");
        let opening_workflow = Arc::clone(&workflow);
        let opening_folder = folder_reference.clone();
        let open_task = smol::spawn(async move {
            opening_workflow
                .open_message(&opening_folder, &message_id())
                .await
        });
        fetch_started
            .recv()
            .await
            .expect("message body request owns its folder reservation");

        assert_eq!(
            workflow.remove_account_cache(&account).await,
            Err(MailCacheError::OperationInProgress)
        );
        release_fetch
            .send(())
            .await
            .expect("synthetic body request is released");
        assert!(
            !open_task
                .await
                .expect("message opens after its body is fetched")
                .loaded_from_cache()
        );
        assert_eq!(body_fetches.load(Ordering::Relaxed), 1);
        assert_eq!(
            cache
                .read_message(&folder_reference, &message_id())
                .await
                .expect("cached message remains readable")
                .and_then(|(_, body)| body),
            Some(body)
        );
        let removal_result = workflow.remove_account_cache(&account).await;
        assert_removal_completed(removal_result, &directory.path().join("mail.sqlite3"));
    });
}

#[test]
fn active_message_move_blocks_cache_removal() {
    smol::block_on(async {
        let directory = TempDir::new().expect("synthetic account directory is created");
        let account = account_id();
        let inbox = folder(&account);
        let archive = archive_folder(&account);
        let inbox_reference = inbox.reference().clone();
        let archive_reference = archive.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let cutoff = started_at - Duration::from_secs(90 * 24 * 60 * 60);
        let source_page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Synthetic subject".to_owned(), cutoff),
            }],
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-move-source"
                    .to_owned(),
            )),
        );
        let destination_page = MessageDeltaPage::new(
            Vec::new(),
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-move-destination"
                    .to_owned(),
            )),
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![inbox, archive],
            [
                Ok(page_response(source_page)),
                Ok(page_response(destination_page)),
            ],
            Err(MailOperationError::ServiceUnavailable),
        );
        let (move_started, release_move) = provider.hold_next_message_move();
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = Arc::new(MailWorkflow::new(provider, cache.clone()));
        workflow
            .synchronize_account(&account, started_at, &ignore_sync_progress)
            .await
            .expect("source and destination folders are cached");
        let moving_workflow = Arc::clone(&workflow);
        let move_task = smol::spawn(async move {
            moving_workflow
                .move_message(&inbox_reference, &message_id(), &archive_reference)
                .await
        });
        move_started
            .recv()
            .await
            .expect("message move holds its folder reservations during provider work");

        assert_eq!(
            workflow.remove_account_cache(&account).await,
            Err(MailCacheError::OperationInProgress)
        );
        release_move
            .send(())
            .await
            .expect("synthetic message move is released");
        assert_eq!(
            move_task.await,
            Ok(einfach_mail_application::MessageMoveOutcome::ConfirmedAndCached)
        );
        let removal_result = workflow.remove_account_cache(&account).await;
        assert_removal_completed(removal_result, &directory.path().join("mail.sqlite3"));
    });
}
