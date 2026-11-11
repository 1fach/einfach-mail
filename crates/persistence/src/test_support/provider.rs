use std::{
    collections::VecDeque,
    future::{Future, ready},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use einfach_mail_application::{
    CalendarDeltaRequest, CalendarDeltaResponse, CalendarOperationError, CalendarProvider,
    CalendarSyncStrategy, MailOperationError, MailProvider, MailSendProviderError,
    MessageDeltaRequest, MessageDeltaResponse, WellKnownMailFolder,
};
use einfach_mail_domain::{
    AccountId, CalendarEventCreation, CalendarEventUpdate, CalendarOccurrence,
    CalendarViewReference, EditableCalendarEvent, FolderId, FolderReference, FollowUpFlagStatus,
    MailDraft, MailFolder, MessageBody, MessageId, ReadState,
};

pub(crate) type PageQueue = VecDeque<Result<MessageDeltaResponse, MailOperationError>>;
pub(crate) type CalendarPageQueue = VecDeque<CalendarDeltaResponse>;
pub(crate) type ProviderGate =
    Arc<Mutex<Option<(smol::channel::Sender<()>, smol::channel::Receiver<()>)>>>;
pub(crate) type ReadStateGate =
    Arc<Mutex<Option<(smol::channel::Sender<()>, smol::channel::Receiver<()>)>>>;
pub(crate) type FollowUpFlagGate =
    Arc<Mutex<Option<(smol::channel::Sender<()>, smol::channel::Receiver<()>)>>>;

#[derive(Clone, Copy)]
pub(crate) enum FollowUpFlagResponse {
    Confirm,
    Deny,
    Unknown,
}

#[derive(Clone)]
pub(crate) struct FixtureProvider {
    pub(crate) account_id: AccountId,
    pub(crate) folders: Vec<MailFolder>,
    pub(crate) folder_list_gate: Option<(smol::channel::Sender<()>, smol::channel::Receiver<()>)>,
    pub(crate) folder_snapshots: Arc<Mutex<VecDeque<Vec<MailFolder>>>>,
    pub(crate) folder_list_calls: Arc<AtomicUsize>,
    pub(crate) pages: Arc<Mutex<PageQueue>>,
    pub(crate) requests: Arc<Mutex<Vec<MessageDeltaRequest>>>,
    pub(crate) body_result: Result<MessageBody, MailOperationError>,
    pub(crate) body_fetches: Arc<AtomicUsize>,
    pub(crate) read_state_updates: Arc<Mutex<Vec<(MessageId, ReadState)>>>,
    pub(crate) read_state_gate: ReadStateGate,
    pub(crate) send_calls: Arc<AtomicUsize>,
    pub(crate) send_result: Result<(), MailSendProviderError>,
    pub(crate) body_gate: Option<BodyFetchGate>,
    pub(crate) delta_gate: Option<DeltaPageGate>,
    pub(crate) follow_up_flag_updates: Arc<Mutex<Vec<(MessageId, FollowUpFlagStatus)>>>,
    pub(crate) follow_up_flag_gate: FollowUpFlagGate,
    pub(crate) follow_up_flag_response: FollowUpFlagResponse,
    pub(crate) message_moves: Arc<Mutex<Vec<(FolderReference, MessageId, FolderReference)>>>,
    pub(crate) move_result: Arc<Mutex<Result<(), MailOperationError>>>,
    pub(crate) move_gate: ProviderGate,
    pub(crate) archive_folder_id: FolderId,
    pub(crate) deleted_items_folder_id: FolderId,
    pub(crate) well_known_folder_calls: Arc<AtomicUsize>,
    pub(crate) well_known_folder_gate: ProviderGate,
    pub(crate) send_gate: ProviderGate,
    pub(crate) calendar_gate: Option<CalendarSyncGate>,
    pub(crate) calendar_pages: Arc<Mutex<CalendarPageQueue>>,
    pub(crate) calendar_delete_result: Result<(), CalendarOperationError>,
    pub(crate) calendar_delete_calls: Arc<AtomicUsize>,
}

#[derive(Clone)]
pub(crate) struct BodyFetchGate {
    pub(crate) started: smol::channel::Sender<()>,
    pub(crate) release: smol::channel::Receiver<()>,
}

#[derive(Clone)]
pub(crate) struct DeltaPageGate {
    pub(crate) request_count: Arc<AtomicUsize>,
    pub(crate) request_index_to_block: usize,
    pub(crate) started: smol::channel::Sender<()>,
    pub(crate) release: smol::channel::Receiver<()>,
}

#[derive(Clone)]
pub(crate) struct CalendarSyncGate {
    pub(crate) started: smol::channel::Sender<()>,
    pub(crate) release: smol::channel::Receiver<()>,
}

impl FixtureProvider {
    pub(crate) fn new(
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

    pub(crate) fn with_calendar_pages(
        mut self,
        pages: impl IntoIterator<Item = CalendarDeltaResponse>,
    ) -> Self {
        self.calendar_pages = Arc::new(Mutex::new(pages.into_iter().collect()));
        self
    }

    pub(crate) fn with_calendar_delete_result(
        mut self,
        result: Result<(), CalendarOperationError>,
    ) -> Self {
        self.calendar_delete_result = result;
        self
    }

    pub(crate) fn hold_next_submission(
        &self,
    ) -> (smol::channel::Receiver<()>, smol::channel::Sender<()>) {
        let (started_tx, started_rx) = smol::channel::bounded(1);
        let (release_tx, release_rx) = smol::channel::bounded(1);
        *self
            .send_gate
            .lock()
            .expect("synthetic send gate mutex is healthy") = Some((started_tx, release_rx));
        (started_rx, release_tx)
    }

    pub(crate) fn hold_next_message_move(
        &self,
    ) -> (smol::channel::Receiver<()>, smol::channel::Sender<()>) {
        let (started_tx, started_rx) = smol::channel::bounded(1);
        let (release_tx, release_rx) = smol::channel::bounded(1);
        *self
            .move_gate
            .lock()
            .expect("synthetic move gate mutex is healthy") = Some((started_tx, release_rx));
        (started_rx, release_tx)
    }

    pub(crate) fn hold_next_well_known_folder_lookup(
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

    pub(crate) fn hold_next_read_state(
        &self,
    ) -> (smol::channel::Receiver<()>, smol::channel::Sender<()>) {
        let (started_tx, started_rx) = smol::channel::bounded(1);
        let (release_tx, release_rx) = smol::channel::bounded(1);
        *self
            .read_state_gate
            .lock()
            .expect("synthetic read-state gate mutex is healthy") = Some((started_tx, release_rx));
        (started_rx, release_tx)
    }

    pub(crate) fn hold_next_follow_up_flag(
        &self,
    ) -> (smol::channel::Receiver<()>, smol::channel::Sender<()>) {
        let (started_tx, started_rx) = smol::channel::bounded(1);
        let (release_tx, release_rx) = smol::channel::bounded(1);
        *self
            .follow_up_flag_gate
            .lock()
            .expect("synthetic flag gate mutex is healthy") = Some((started_tx, release_rx));
        (started_rx, release_tx)
    }

    pub(crate) fn with_send_result(mut self, result: Result<(), MailSendProviderError>) -> Self {
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
    fn calendar_sync_strategy(
        &self,
    ) -> impl Future<Output = Result<CalendarSyncStrategy, CalendarOperationError>> + Send + '_
    {
        ready(Ok(CalendarSyncStrategy::Delta))
    }

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
