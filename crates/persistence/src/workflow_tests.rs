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
    AccountCache, AccountCacheFactory, AccountCacheOpenError, AccountCacheRemovalError,
    AccountSession, AccountSessionFactory, AccountSessionWorkflowFactory,
    CalendarDeltaPageResponse, CalendarDeltaRequest, CalendarDeltaResponse, CalendarOperationError,
    CalendarProvider, CalendarWorkflowError, MailCache, MailCacheError, MailOperationError,
    MailProvider, MailSendOutcome, MailSendProviderError, MailSendResolutionError,
    MailSendWorkflowError, MailSyncProgress, MailWorkflow, MailWorkflowError,
    MessageBodyForDisplay, MessageDeltaPageResponse, MessageDeltaRequest, MessageDeltaResponse,
    WellKnownMailFolder,
};
use einfach_mail_domain::{
    AccountId, CalendarDeltaLink, CalendarDeltaPage, CalendarDeltaProgress, CalendarEventCreation,
    CalendarEventUpdate, CalendarEventVersion, CalendarOccurrence, CalendarOccurrencePatch,
    CalendarOccurrenceTimePatch, CalendarViewRange, CalendarViewReference, EditableCalendarEvent,
    EventId, FolderId, FolderReference, FollowUpFlagStatus, MailDraft, MailFolder, MailRecipient,
    MailReplyDraft, MailReplySource, MailSendOperationId, MailSubject, MessageBody,
    MessageBodyType, MessageDeltaChange, MessageDeltaLink, MessageDeltaPage, MessageDeltaProgress,
    MessageId, MessageMetadataPatch, ReadState,
};
use tempfile::TempDir;
use time::{Date, Month};

use crate::store::SqliteMailCache;

mod account_lifecycle;
mod mail_actions;
mod mail_sync;

const DATABASE_KEY_BYTE: u8 = 0x62;

type PageQueue = VecDeque<Result<MessageDeltaResponse, MailOperationError>>;
type CalendarPageQueue = VecDeque<CalendarDeltaResponse>;
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
    calendar_pages: Arc<Mutex<CalendarPageQueue>>,
    calendar_delete_result: Result<(), CalendarOperationError>,
    calendar_delete_calls: Arc<AtomicUsize>,
}

struct FixtureCacheFactory {
    database_directory: std::path::PathBuf,
}

struct RetainedCacheFactory {
    account_id: AccountId,
    cache: SqliteMailCache,
}

impl AccountCacheFactory for FixtureCacheFactory {
    type Cache = SqliteMailCache;

    fn open_for_account(
        &self,
        account_id: AccountId,
    ) -> impl Future<Output = Result<Self::Cache, AccountCacheOpenError>> + Send + '_ {
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
    ) -> impl Future<Output = Result<(), AccountCacheRemovalError>> + Send + 'a {
        ready(Err(AccountCacheRemovalError::Unavailable))
    }
}

impl AccountCacheFactory for RetainedCacheFactory {
    type Cache = SqliteMailCache;

    fn open_for_account(
        &self,
        account_id: AccountId,
    ) -> impl Future<Output = Result<Self::Cache, AccountCacheOpenError>> + Send + '_ {
        ready(if account_id == self.account_id {
            Ok(self.cache.clone())
        } else {
            Err(AccountCacheOpenError::AccountMismatch)
        })
    }

    fn remove_for_account<'a>(
        &'a self,
        _: &'a AccountId,
    ) -> impl Future<Output = Result<(), AccountCacheRemovalError>> + Send + 'a {
        ready(Err(AccountCacheRemovalError::Unavailable))
    }
}

#[derive(Clone)]
struct BodyFetchGate {
    started: smol::channel::Sender<()>,
    release: smol::channel::Receiver<()>,
}

#[derive(Clone)]
struct DeltaPageGate {
    request_count: Arc<AtomicUsize>,
    request_index_to_block: usize,
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
            calendar_pages: Arc::new(Mutex::new(VecDeque::new())),
            calendar_delete_result: Err(CalendarOperationError::ServiceUnavailable),
            calendar_delete_calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn with_calendar_pages(
        mut self,
        pages: impl IntoIterator<Item = CalendarDeltaResponse>,
    ) -> Self {
        self.calendar_pages = Arc::new(Mutex::new(pages.into_iter().collect()));
        self
    }

    fn with_calendar_delete_result(mut self, result: Result<(), CalendarOperationError>) -> Self {
        self.calendar_delete_result = result;
        self
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
                && gate.request_count.fetch_add(1, Ordering::Relaxed) == gate.request_index_to_block
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
                started
                    .send(())
                    .await
                    .map_err(|_| MailOperationError::ServiceUnavailable)?;
                release
                    .recv()
                    .await
                    .map_err(|_| MailOperationError::ServiceUnavailable)?;
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
                started
                    .send(())
                    .await
                    .map_err(|_| MailSendProviderError::ServiceUnavailable)?;
                release
                    .recv()
                    .await
                    .map_err(|_| MailSendProviderError::ServiceUnavailable)?;
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
                started
                    .send(())
                    .await
                    .map_err(|_| MailSendProviderError::ServiceUnavailable)?;
                release
                    .recv()
                    .await
                    .map_err(|_| MailSendProviderError::ServiceUnavailable)?;
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
        let result = self.calendar_delete_result;
        let calls = Arc::clone(&self.calendar_delete_calls);
        async move {
            calls.fetch_add(1, Ordering::Relaxed);
            result
        }
    }

    fn calendar_delta_page(
        &self,
        view: &CalendarViewReference,
        _request: CalendarDeltaRequest,
    ) -> impl Future<Output = Result<CalendarDeltaResponse, CalendarOperationError>> + Send + '_
    {
        let matches_account = view.account_id() == &self.account_id;
        let gate = self.calendar_gate.clone();
        let pages = Arc::clone(&self.calendar_pages);
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
            pages
                .lock()
                .expect("synthetic calendar page mutex is not poisoned")
                .pop_front()
                .ok_or(CalendarOperationError::ServiceUnavailable)
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

fn completed_calendar_page(cursor: &str) -> CalendarDeltaResponse {
    CalendarDeltaResponse::Page(CalendarDeltaPageResponse::new(
        CalendarDeltaPage::new(
            Vec::new(),
            CalendarDeltaProgress::Complete(CalendarDeltaLink::from_opaque(format!(
                "https://graph.microsoft.com/v1.0/me/calendarView/delta?$deltatoken={cursor}"
            ))),
        ),
        128,
    ))
}

fn editable_calendar_event(view: CalendarViewReference) -> EditableCalendarEvent {
    let mut patch = CalendarOccurrencePatch::new();
    patch.set_subject(Some("Synthetic event".to_owned()));
    let start = view.range().start() + time::Duration::hours(1);
    let mut time = CalendarOccurrenceTimePatch::new();
    time.set_start(start);
    time.set_end(start + time::Duration::hours(1));
    time.set_is_all_day(false);
    patch.set_time(time);
    let occurrence = patch
        .into_new(
            view,
            EventId::try_from("synthetic-event".to_owned()).expect("synthetic event ID is valid"),
        )
        .expect("synthetic calendar event is complete");
    let version = CalendarEventVersion::try_from("synthetic-version".to_owned())
        .expect("synthetic calendar version is valid");
    EditableCalendarEvent::new(occurrence, version)
        .expect("synthetic timed calendar event is editable")
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

fn assert_removal_completed(
    result: Result<(), AccountCacheRemovalError>,
    database_path: &std::path::Path,
) {
    assert_ne!(result, Err(AccountCacheRemovalError::OperationInProgress));
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

async fn open_session_over_retained_cache(
    provider: FixtureProvider,
    account_id: &AccountId,
    cache: &SqliteMailCache,
) -> Arc<dyn AccountSession> {
    let factory = AccountSessionWorkflowFactory::new(
        provider,
        RetainedCacheFactory {
            account_id: account_id.clone(),
            cache: cache.clone(),
        },
    );
    factory
        .open_session(account_id.clone())
        .await
        .expect("synthetic account session opens over its retained cache")
}
