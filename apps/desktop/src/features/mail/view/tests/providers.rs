use super::*;

#[derive(Clone)]
pub(super) struct OfflineProvider {
    pub(super) body_fetches: Arc<AtomicUsize>,
}

impl MailProvider for OfflineProvider {
    fn list_folders(
        &self,
        _: &AccountId,
    ) -> impl Future<Output = Result<Vec<MailFolder>, MailOperationError>> + Send + '_ {
        ready(Err(MailOperationError::ServiceUnavailable))
    }

    fn message_delta_page(
        &self,
        _: &FolderReference,
        _: MessageDeltaRequest,
    ) -> impl Future<Output = Result<MessageDeltaResponse, MailOperationError>> + Send + '_ {
        ready(Err(MailOperationError::ServiceUnavailable))
    }

    fn fetch_message_body(
        &self,
        _: &FolderReference,
        _: &MessageId,
    ) -> impl Future<Output = Result<MessageBody, MailOperationError>> + Send + '_ {
        self.body_fetches.fetch_add(1, Ordering::Relaxed);
        ready(Err(MailOperationError::ServiceUnavailable))
    }

    fn set_message_read_state(
        &self,
        _: &FolderReference,
        _: &MessageId,
        _: ReadState,
    ) -> impl Future<Output = Result<ReadState, MailOperationError>> + Send + '_ {
        ready(Err(MailOperationError::ServiceUnavailable))
    }

    fn set_message_follow_up_flag_status(
        &self,
        _: &FolderReference,
        _: &MessageId,
        _: FollowUpFlagStatus,
    ) -> impl Future<Output = Result<FollowUpFlagStatus, MailOperationError>> + Send + '_ {
        ready(Err(MailOperationError::ServiceUnavailable))
    }

    fn move_message(
        &self,
        _: &FolderReference,
        _: &MessageId,
        _: &FolderReference,
    ) -> impl Future<Output = Result<(), MailOperationError>> + Send + '_ {
        ready(Err(MailOperationError::ServiceUnavailable))
    }

    fn well_known_folder(
        &self,
        account_id: &AccountId,
        folder: WellKnownMailFolder,
    ) -> impl Future<Output = Result<FolderReference, MailOperationError>> + Send + '_ {
        let folder_id = match folder {
            WellKnownMailFolder::Archive => "archive",
            WellKnownMailFolder::DeletedItems => "deleted-items",
        };
        ready(Ok(FolderReference::new(
            account_id.clone(),
            FolderId::try_from(folder_id.to_owned())
                .expect("synthetic well-known folder ID is valid"),
        )))
    }

    fn send_mail(
        &self,
        _: &AccountId,
        _: MailDraft,
    ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_ {
        ready(Err(MailSendProviderError::ServiceUnavailable))
    }

    fn reply_to_message(
        &self,
        _: &AccountId,
        _: MailReplyDraft,
    ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_ {
        ready(Err(MailSendProviderError::ServiceUnavailable))
    }
}

#[derive(Clone, Copy)]
pub(super) enum ReadStateActionResponse {
    Confirm,
    Unconfirmed,
    Pending,
    Deny,
    Offline,
}

pub(super) struct ReadStateProvider {
    response: ReadStateActionResponse,
    updates: Arc<Mutex<Vec<(MessageId, ReadState)>>>,
    flag_updates: Arc<Mutex<Vec<(MessageId, FollowUpFlagStatus)>>>,
    sync_failure: bool,
    folders: Option<Vec<MailFolder>>,
    sync_gate: Mutex<Option<SyncGate>>,
    offline: OfflineProvider,
}

struct SyncGate {
    started: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
}

impl ReadStateProvider {
    pub(super) fn new(
        response: ReadStateActionResponse,
        updates: Arc<Mutex<Vec<(MessageId, ReadState)>>>,
    ) -> Self {
        Self {
            response,
            updates,
            flag_updates: Arc::new(Mutex::new(Vec::new())),
            sync_failure: false,
            folders: None,
            sync_gate: Mutex::new(None),
            offline: OfflineProvider {
                body_fetches: Arc::new(AtomicUsize::new(0)),
            },
        }
    }

    pub(super) fn with_folders(mut self, folders: Vec<MailFolder>) -> Self {
        self.folders = Some(folders);
        self
    }

    pub(super) fn hold_sync_until_released(
        mut self,
        started: tokio::sync::oneshot::Sender<()>,
        release: tokio::sync::oneshot::Receiver<()>,
    ) -> Self {
        self.sync_gate = Mutex::new(Some(SyncGate { started, release }));
        self
    }

    pub(super) fn with_flag_updates(
        mut self,
        updates: Arc<Mutex<Vec<(MessageId, FollowUpFlagStatus)>>>,
    ) -> Self {
        self.flag_updates = updates;
        self
    }

    pub(super) fn with_sync_failure(mut self) -> Self {
        self.sync_failure = true;
        self
    }
}

impl MailProvider for ReadStateProvider {
    fn list_folders(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<Vec<MailFolder>, MailOperationError>> + Send + '_ {
        let account_id = account_id.clone();
        let sync_failure = self.sync_failure;
        let folders = self.folders.clone();
        let sync_gate = self
            .sync_gate
            .lock()
            .expect("fixture sync-gate mutex is healthy")
            .take();
        async move {
            if sync_failure {
                return Err(MailOperationError::ServiceUnavailable);
            }
            if let Some(sync_gate) = sync_gate {
                sync_gate
                    .started
                    .send(())
                    .expect("the mail sync test is waiting at its provider gate");
                sync_gate
                    .release
                    .await
                    .expect("the mail sync test releases its provider gate");
            }
            if let Some(folders) = folders {
                return Ok(folders);
            }
            let reference = FolderReference::new(
                account_id,
                FolderId::try_from("inbox".to_owned()).expect("synthetic folder ID is valid"),
            );
            Ok(vec![
                MailFolder::new(reference, None, "Inbox".to_owned())
                    .expect("synthetic folder is valid"),
            ])
        }
    }

    fn message_delta_page(
        &self,
        _: &FolderReference,
        _: MessageDeltaRequest,
    ) -> impl Future<Output = Result<MessageDeltaResponse, MailOperationError>> + Send + '_ {
        let sync_failure = self.sync_failure;
        async move {
            if sync_failure {
                return Err(MailOperationError::ServiceUnavailable);
            }
            let page = MessageDeltaPage::new(
                Vec::new(),
                MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                    "https://graph.microsoft.com/v1.0/delta?token=synthetic".to_owned(),
                )),
            );
            Ok(MessageDeltaResponse::Page(MessageDeltaPageResponse::new(
                page, 0,
            )))
        }
    }

    fn fetch_message_body(
        &self,
        _: &FolderReference,
        _: &MessageId,
    ) -> impl Future<Output = Result<MessageBody, MailOperationError>> + Send + '_ {
        ready(Err(MailOperationError::ServiceUnavailable))
    }

    fn set_message_read_state(
        &self,
        _: &FolderReference,
        message_id: &MessageId,
        requested: ReadState,
    ) -> impl Future<Output = Result<ReadState, MailOperationError>> + Send + '_ {
        let message_id = message_id.clone();
        let response = self.response;
        let updates = Arc::clone(&self.updates);
        async move {
            updates
                .lock()
                .expect("fixture read-state log mutex is healthy")
                .push((message_id, requested));
            match response {
                ReadStateActionResponse::Confirm => Ok(requested),
                ReadStateActionResponse::Unconfirmed => {
                    Err(MailOperationError::MessageUpdateOutcomeUnknown)
                }
                ReadStateActionResponse::Pending => {
                    pending::<Result<ReadState, MailOperationError>>().await
                }
                ReadStateActionResponse::Deny => Err(MailOperationError::AccessDenied),
                ReadStateActionResponse::Offline => Err(MailOperationError::ServiceUnavailable),
            }
        }
    }

    fn set_message_follow_up_flag_status(
        &self,
        _: &FolderReference,
        message_id: &MessageId,
        status: FollowUpFlagStatus,
    ) -> impl Future<Output = Result<FollowUpFlagStatus, MailOperationError>> + Send + '_ {
        let message_id = message_id.clone();
        let updates = Arc::clone(&self.flag_updates);
        let response = self.response;
        async move {
            updates
                .lock()
                .expect("fixture flag-update log mutex is healthy")
                .push((message_id, status));
            match response {
                ReadStateActionResponse::Confirm => Ok(status),
                ReadStateActionResponse::Unconfirmed => {
                    Err(MailOperationError::MessageUpdateOutcomeUnknown)
                }
                ReadStateActionResponse::Pending => {
                    pending::<Result<FollowUpFlagStatus, MailOperationError>>().await
                }
                ReadStateActionResponse::Deny => Err(MailOperationError::AccessDenied),
                ReadStateActionResponse::Offline => Err(MailOperationError::ServiceUnavailable),
            }
        }
    }

    fn move_message(
        &self,
        _: &FolderReference,
        _: &MessageId,
        _: &FolderReference,
    ) -> impl Future<Output = Result<(), MailOperationError>> + Send + '_ {
        ready(Err(MailOperationError::ServiceUnavailable))
    }

    fn well_known_folder(
        &self,
        account_id: &AccountId,
        folder: WellKnownMailFolder,
    ) -> impl Future<Output = Result<FolderReference, MailOperationError>> + Send + '_ {
        self.offline.well_known_folder(account_id, folder)
    }

    fn send_mail(
        &self,
        account_id: &AccountId,
        draft: MailDraft,
    ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_ {
        self.offline.send_mail(account_id, draft)
    }

    fn reply_to_message(
        &self,
        account_id: &AccountId,
        draft: MailReplyDraft,
    ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_ {
        self.offline.reply_to_message(account_id, draft)
    }
}

#[derive(Clone, Copy)]
pub(super) enum MessageMoveActionResponse {
    Confirm,
    AccessDenied,
    Unknown,
    Pending,
}

#[derive(Clone)]
pub(super) struct MessageMoveProvider {
    pub(super) account_id: AccountId,
    pub(super) folders: Vec<MailFolder>,
    pub(super) offline: OfflineProvider,
    pub(super) response: MessageMoveActionResponse,
    pub(super) calls: Arc<AtomicUsize>,
}

impl MailProvider for MessageMoveProvider {
    fn list_folders(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<Vec<MailFolder>, MailOperationError>> + Send + '_ {
        let matches_account = account_id == &self.account_id;
        let folders = self.folders.clone();
        async move {
            if matches_account {
                Ok(folders)
            } else {
                Err(MailOperationError::AccountMismatch)
            }
        }
    }

    fn message_delta_page(
        &self,
        folder: &FolderReference,
        _: MessageDeltaRequest,
    ) -> impl Future<Output = Result<MessageDeltaResponse, MailOperationError>> + Send + '_ {
        let matches_account = folder.account_id() == &self.account_id;
        async move {
            if !matches_account {
                return Err(MailOperationError::AccountMismatch);
            }
            let progress = MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-move-view"
                    .to_owned(),
            ));
            Ok(MessageDeltaResponse::Page(MessageDeltaPageResponse::new(
                MessageDeltaPage::new(Vec::new(), progress),
                1_024,
            )))
        }
    }

    fn fetch_message_body(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
    ) -> impl Future<Output = Result<MessageBody, MailOperationError>> + Send + '_ {
        self.offline.fetch_message_body(folder, message_id)
    }

    fn set_message_read_state(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
        read_state: ReadState,
    ) -> impl Future<Output = Result<ReadState, MailOperationError>> + Send + '_ {
        self.offline
            .set_message_read_state(folder, message_id, read_state)
    }

    fn set_message_follow_up_flag_status(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
        status: FollowUpFlagStatus,
    ) -> impl Future<Output = Result<FollowUpFlagStatus, MailOperationError>> + Send + '_ {
        self.offline
            .set_message_follow_up_flag_status(folder, message_id, status)
    }

    fn move_message(
        &self,
        source_folder: &FolderReference,
        _: &MessageId,
        destination_folder: &FolderReference,
    ) -> impl Future<Output = Result<(), MailOperationError>> + Send + '_ {
        let account_matches = source_folder.account_id() == destination_folder.account_id();
        let calls = Arc::clone(&self.calls);
        let response = self.response;
        async move {
            if !account_matches {
                return Err(MailOperationError::AccountMismatch);
            }
            calls.fetch_add(1, Ordering::Relaxed);
            match response {
                MessageMoveActionResponse::Confirm => Ok(()),
                MessageMoveActionResponse::AccessDenied => Err(MailOperationError::AccessDenied),
                MessageMoveActionResponse::Unknown => Err(MailOperationError::MoveOutcomeUnknown),
                MessageMoveActionResponse::Pending => std::future::pending().await,
            }
        }
    }

    fn well_known_folder(
        &self,
        account_id: &AccountId,
        folder: WellKnownMailFolder,
    ) -> impl Future<Output = Result<FolderReference, MailOperationError>> + Send + '_ {
        let folder_id = match folder {
            WellKnownMailFolder::Archive => "archive",
            WellKnownMailFolder::DeletedItems => "deleted-items",
        };
        if account_id == &self.account_id {
            ready(Ok(FolderReference::new(
                account_id.clone(),
                FolderId::try_from(folder_id.to_owned())
                    .expect("synthetic well-known folder ID is valid"),
            )))
        } else {
            ready(Err(MailOperationError::AccountMismatch))
        }
    }

    fn send_mail(
        &self,
        _: &AccountId,
        _: MailDraft,
    ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_ {
        ready(Err(MailSendProviderError::ServiceUnavailable))
    }

    fn reply_to_message(
        &self,
        account_id: &AccountId,
        draft: MailReplyDraft,
    ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_ {
        self.offline.reply_to_message(account_id, draft)
    }
}

pub(super) struct SendGate {
    started: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
}

#[derive(Clone)]
pub(super) struct PermissionProvider {
    pub(super) offline: OfflineProvider,
    pub(super) can_send: Arc<AtomicBool>,
    pub(super) send_calls: Arc<AtomicUsize>,
    pub(super) send_gate: Arc<Mutex<Option<SendGate>>>,
    pub(super) reply_sources: Arc<Mutex<Vec<(String, String, String)>>>,
    pub(super) reply_error: Arc<Mutex<Option<MailSendProviderError>>>,
}

impl PermissionProvider {
    pub(super) fn hold_next_send(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (started, started_receiver) = tokio::sync::oneshot::channel();
        let (release, release_receiver) = tokio::sync::oneshot::channel();
        *self
            .send_gate
            .lock()
            .expect("fixture send-gate mutex is healthy") = Some(SendGate {
            started,
            release: release_receiver,
        });
        (started_receiver, release)
    }
}

impl MailProvider for PermissionProvider {
    fn list_folders(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<Vec<MailFolder>, MailOperationError>> + Send + '_ {
        self.offline.list_folders(account_id)
    }

    fn message_delta_page(
        &self,
        folder: &FolderReference,
        request: MessageDeltaRequest,
    ) -> impl Future<Output = Result<MessageDeltaResponse, MailOperationError>> + Send + '_ {
        self.offline.message_delta_page(folder, request)
    }

    fn fetch_message_body(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
    ) -> impl Future<Output = Result<MessageBody, MailOperationError>> + Send + '_ {
        self.offline.fetch_message_body(folder, message_id)
    }

    fn set_message_read_state(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
        requested: ReadState,
    ) -> impl Future<Output = Result<ReadState, MailOperationError>> + Send + '_ {
        self.offline
            .set_message_read_state(folder, message_id, requested)
    }

    fn set_message_follow_up_flag_status(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
        status: FollowUpFlagStatus,
    ) -> impl Future<Output = Result<FollowUpFlagStatus, MailOperationError>> + Send + '_ {
        self.offline
            .set_message_follow_up_flag_status(folder, message_id, status)
    }

    fn move_message(
        &self,
        source_folder: &FolderReference,
        message_id: &MessageId,
        destination_folder: &FolderReference,
    ) -> impl Future<Output = Result<(), MailOperationError>> + Send + '_ {
        self.offline
            .move_message(source_folder, message_id, destination_folder)
    }

    fn well_known_folder(
        &self,
        account_id: &AccountId,
        folder: WellKnownMailFolder,
    ) -> impl Future<Output = Result<FolderReference, MailOperationError>> + Send + '_ {
        self.offline.well_known_folder(account_id, folder)
    }

    fn send_mail(
        &self,
        account_id: &AccountId,
        _: MailDraft,
    ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_ {
        let matches_account = account_id.as_str() == "synthetic-account";
        let can_send = Arc::clone(&self.can_send);
        let send_calls = Arc::clone(&self.send_calls);
        let send_gate = self
            .send_gate
            .lock()
            .expect("fixture send-gate mutex is healthy")
            .take();
        async move {
            if !matches_account {
                return Err(MailSendProviderError::AccountMismatch);
            }
            send_calls.fetch_add(1, Ordering::Relaxed);
            if let Some(gate) = send_gate {
                gate.started
                    .send(())
                    .expect("the mail send test is waiting at its provider gate");
                gate.release
                    .await
                    .expect("the mail send test releases its provider gate");
            }
            if can_send.load(Ordering::Relaxed) {
                Ok(())
            } else {
                Err(MailSendProviderError::PermissionRequired)
            }
        }
    }

    fn reply_to_message(
        &self,
        account_id: &AccountId,
        draft: MailReplyDraft,
    ) -> impl Future<Output = Result<(), MailSendProviderError>> + Send + '_ {
        let matches_account =
            account_id.as_str() == "synthetic-account" && draft.source().account_id() == account_id;
        let can_send = Arc::clone(&self.can_send);
        let send_calls = Arc::clone(&self.send_calls);
        let reply_sources = Arc::clone(&self.reply_sources);
        let reply_error = self
            .reply_error
            .lock()
            .expect("fixture reply-error mutex is healthy")
            .take();
        let source = draft.source();
        let account_id = account_id.as_str().to_owned();
        let folder_id = source.folder().folder_id().as_str().to_owned();
        let message_id = source.message_id().as_str().to_owned();
        let send_gate = self
            .send_gate
            .lock()
            .expect("fixture send-gate mutex is healthy")
            .take();
        async move {
            if !matches_account {
                return Err(MailSendProviderError::AccountMismatch);
            }
            send_calls.fetch_add(1, Ordering::Relaxed);
            reply_sources
                .lock()
                .expect("fixture reply-source mutex is healthy")
                .push((account_id, folder_id, message_id));
            if let Some(gate) = send_gate {
                gate.started
                    .send(())
                    .expect("the mail reply test is waiting at its provider gate");
                gate.release
                    .await
                    .expect("the mail reply test releases its provider gate");
            }
            if let Some(error) = reply_error {
                Err(error)
            } else if can_send.load(Ordering::Relaxed) {
                Ok(())
            } else {
                Err(MailSendProviderError::PermissionRequired)
            }
        }
    }
}

pub(super) struct PermissionGate {
    pub(super) started: tokio::sync::oneshot::Sender<()>,
    pub(super) release: tokio::sync::oneshot::Receiver<()>,
}

pub(super) struct FixturePermissionRequester {
    pub(super) account_id: AccountId,
    pub(super) requests: Arc<AtomicUsize>,
    pub(super) gate: Mutex<Option<PermissionGate>>,
}

impl MailPermissionRequester for FixturePermissionRequester {
    fn request_send_permission<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> Pin<Box<dyn Future<Output = Result<(), AccountOperationError>> + Send + 'a>> {
        let matches_account = account_id == &self.account_id;
        let requests = Arc::clone(&self.requests);
        let gate = self
            .gate
            .lock()
            .expect("fixture permission-gate mutex is healthy")
            .take();
        Box::pin(async move {
            if !matches_account {
                return Err(AccountOperationError::AccountMismatch);
            }
            requests.fetch_add(1, Ordering::Relaxed);
            if let Some(gate) = gate {
                gate.started
                    .send(())
                    .expect("the permission review test is waiting at its requester gate");
                gate.release
                    .await
                    .expect("the permission review test releases its requester gate");
            }
            Ok(())
        })
    }
}
