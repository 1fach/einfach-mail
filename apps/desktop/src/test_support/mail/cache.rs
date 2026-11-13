use std::{
    future::{Future, ready},
    sync::{Arc, Mutex},
    time::SystemTime,
};

use einfach_mail_application::{
    AccountCache, AccountCacheRemovalError, MailCache, MailCacheError, MailSyncState,
};
use einfach_mail_domain::{
    AccountId, FolderReference, FollowUpFlagStatus, MailFolder, MailMessage, MailSendOperationId,
    MessageBody, MessageDeltaLink, MessageDeltaPage, MessageDeltaProgress, MessageId,
    MessageMetadataPatch, PendingMailSend, ReadState,
};

#[cfg(test)]
use super::super::synthetic_account_profile;
#[cfg(test)]
use einfach_mail_domain::{AccountCapabilities, AccountProfile};
#[cfg(test)]
use einfach_mail_domain::{FolderId, Mailbox, MessageBodyType};
#[cfg(test)]
use std::time::{Duration, UNIX_EPOCH};

#[derive(Clone, Default)]
pub(crate) struct FixtureCache(pub(crate) Arc<Mutex<FixtureData>>);

#[derive(Default)]
pub(crate) struct FixtureData {
    pub(crate) account_id: Option<AccountId>,
    pub(crate) folders: Vec<MailFolder>,
    pub(crate) messages: Vec<MailMessage>,
    pub(crate) list_folders_error: Option<MailCacheError>,
    list_folders_gate: Option<FolderListGate>,
    pub(crate) body: Option<MessageBody>,
    pub(crate) list_messages_error: Option<MailCacheError>,
    pub(crate) sync_state: Option<MailSyncState>,
    pub(crate) read_state_write_error: Option<MailCacheError>,
    pub(crate) folder_write_sync_required: bool,
    pub(crate) pending_send: Option<PendingMailSend>,
    pub(crate) fail_next_pending_send_stage: bool,
    pub(crate) fail_next_pending_send_clear: bool,
    pub(crate) move_error: Option<MailCacheError>,
}

struct FolderListGate {
    started: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
}

impl FixtureCache {
    #[cfg(test)]
    pub(crate) fn hold_next_folder_list(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (started, started_receiver) = tokio::sync::oneshot::channel();
        let (release, release_receiver) = tokio::sync::oneshot::channel();
        self.0
            .lock()
            .expect("fixture cache mutex is healthy")
            .list_folders_gate = Some(FolderListGate {
            started,
            release: release_receiver,
        });
        (started_receiver, release)
    }
}

impl MailCache for FixtureCache {
    fn replace_folders(
        &self,
        account_id: &AccountId,
        folders: Vec<MailFolder>,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
        let account_id = account_id.clone();
        async move {
            let mut data = self.0.lock().expect("fixture cache mutex is healthy");
            if data
                .account_id
                .as_ref()
                .is_some_and(|saved| saved != &account_id)
            {
                return Err(MailCacheError::AccountMismatch);
            }
            data.account_id = Some(account_id);
            data.folders = folders;
            Ok(())
        }
    }

    fn ensure_account_sync_scope(
        &self,
        _: &AccountId,
        _: SystemTime,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
        ready(Ok(()))
    }

    fn ensure_sync_state(
        &self,
        _: &FolderReference,
        initial_cutoff: SystemTime,
    ) -> impl Future<Output = Result<MailSyncState, MailCacheError>> + Send + '_ {
        let mut data = self.0.lock().expect("fixture cache mutex is healthy");
        ready(Ok(data
            .sync_state
            .get_or_insert_with(|| MailSyncState::initial(initial_cutoff))
            .clone()))
    }

    fn load_sync_state(
        &self,
        _: &FolderReference,
    ) -> impl Future<Output = Result<Option<MailSyncState>, MailCacheError>> + Send + '_ {
        ready(Ok(self
            .0
            .lock()
            .expect("fixture cache mutex is healthy")
            .sync_state
            .clone()))
    }

    fn commit_delta_page(
        &self,
        _: &FolderReference,
        expected_state: &MailSyncState,
        page: MessageDeltaPage,
        completed_at: SystemTime,
    ) -> impl Future<Output = Result<MailSyncState, MailCacheError>> + Send + '_ {
        let changes_empty = page.changes().is_empty();
        let progress = page.into_parts().1;
        let mut data = self.0.lock().expect("fixture cache mutex is healthy");
        let Some(current) = data.sync_state.as_ref() else {
            return ready(Err(MailCacheError::InvalidData));
        };
        if current != expected_state {
            return ready(Err(MailCacheError::SyncStateChanged));
        }
        let MessageDeltaProgress::Complete(delta_link) = progress else {
            return ready(Err(MailCacheError::InvalidData));
        };
        if !changes_empty {
            return ready(Err(MailCacheError::InvalidData));
        }
        let Some(checkpoint_revision) = current.checkpoint_revision().checked_add(1) else {
            return ready(Err(MailCacheError::InvalidData));
        };
        let state = MailSyncState::from_checkpoint(
            current.cutoff(),
            Some(delta_link),
            None,
            Some(completed_at),
            current.full_sync_generation(),
            checkpoint_revision,
        );
        match state {
            Ok(state) => {
                data.sync_state = Some(state.clone());
                data.folder_write_sync_required = false;
                ready(Ok(state))
            }
            Err(error) => ready(Err(error)),
        }
    }

    fn begin_full_resync(
        &self,
        _: &FolderReference,
        expected_state: &MailSyncState,
        reset_link: MessageDeltaLink,
    ) -> impl Future<Output = Result<MailSyncState, MailCacheError>> + Send + '_ {
        let mut data = self.0.lock().expect("fixture cache mutex is healthy");
        let Some(current) = data.sync_state.as_ref() else {
            return ready(Err(MailCacheError::InvalidData));
        };
        if current != expected_state {
            return ready(Err(MailCacheError::SyncStateChanged));
        }
        let Some(generation) = current.full_sync_generation().unwrap_or(0).checked_add(1) else {
            return ready(Err(MailCacheError::InvalidData));
        };
        let Some(checkpoint_revision) = current.checkpoint_revision().checked_add(1) else {
            return ready(Err(MailCacheError::InvalidData));
        };
        let state = MailSyncState::from_checkpoint(
            current.cutoff(),
            current.delta_link().cloned(),
            Some(reset_link),
            current.last_synchronized_at(),
            Some(generation),
            checkpoint_revision,
        );
        match state {
            Ok(state) => {
                data.sync_state = Some(state.clone());
                ready(Ok(state))
            }
            Err(error) => ready(Err(error)),
        }
    }

    fn list_folders(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<Vec<MailFolder>, MailCacheError>> + Send + '_ {
        let account_id = account_id.clone();
        async move {
            let (folders, error, gate) = {
                let mut data = self.0.lock().expect("fixture cache mutex is healthy");
                if data
                    .account_id
                    .as_ref()
                    .is_some_and(|saved| saved != &account_id)
                {
                    return Err(MailCacheError::AccountMismatch);
                }
                (
                    data.folders.clone(),
                    data.list_folders_error,
                    data.list_folders_gate.take(),
                )
            };
            if let Some(gate) = gate {
                gate.started
                    .send(())
                    .expect("the folder-list test is waiting at its cache gate");
                gate.release
                    .await
                    .expect("the folder-list test releases its cache gate");
            }
            if let Some(error) = error {
                return Err(error);
            }
            Ok(folders)
        }
    }

    fn list_messages(
        &self,
        folder: &FolderReference,
    ) -> impl Future<Output = Result<Vec<MailMessage>, MailCacheError>> + Send + '_ {
        let folder = folder.clone();
        let data = self.0.lock().expect("fixture cache mutex is healthy");
        if let Some(error) = data.list_messages_error {
            return ready(Err(error));
        }
        let messages = data
            .messages
            .iter()
            .filter(|message| message.folder() == &folder)
            .cloned()
            .collect();
        ready(Ok(messages))
    }

    fn read_message(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
    ) -> impl Future<Output = Result<Option<(MailMessage, Option<MessageBody>)>, MailCacheError>>
    + Send
    + '_ {
        let message_id = message_id.clone();
        let folder = folder.clone();
        async move {
            let data = self.0.lock().expect("fixture cache mutex is healthy");
            Ok(data
                .messages
                .iter()
                .find(|message| message.folder() == &folder && message.id() == &message_id)
                .cloned()
                .map(|message| (message, data.body.clone())))
        }
    }

    fn message_read_state(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
    ) -> impl Future<Output = Result<Option<ReadState>, MailCacheError>> + Send + '_ {
        let folder = folder.clone();
        let message_id = message_id.clone();
        async move {
            let data = self.0.lock().expect("fixture cache mutex is healthy");
            if data
                .account_id
                .as_ref()
                .is_some_and(|account_id| account_id != folder.account_id())
            {
                return Err(MailCacheError::AccountMismatch);
            }
            Ok(data
                .messages
                .iter()
                .find(|message| message.folder() == &folder && message.id() == &message_id)
                .map(|message| message.metadata().read_state()))
        }
    }

    fn set_message_read_state(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
        read_state: ReadState,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
        let folder = folder.clone();
        let message_id = message_id.clone();
        async move {
            let mut data = self.0.lock().expect("fixture cache mutex is healthy");
            if let Some(error) = data.read_state_write_error {
                return Err(error);
            }
            let Some(message) = data
                .messages
                .iter_mut()
                .find(|message| message.folder() == &folder && message.id() == &message_id)
            else {
                return Err(MailCacheError::MessageNotCached);
            };
            let mut patch = MessageMetadataPatch::new();
            patch.set_read_state(read_state);
            message.apply_patch(patch);
            data.folder_write_sync_required = false;
            Ok(())
        }
    }

    fn folder_write_sync_required(
        &self,
        _: &FolderReference,
    ) -> impl Future<Output = Result<bool, MailCacheError>> + Send + '_ {
        ready(Ok(self
            .0
            .lock()
            .expect("fixture cache mutex is healthy")
            .folder_write_sync_required))
    }

    fn require_folder_write_sync(
        &self,
        _: &FolderReference,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
        self.0
            .lock()
            .expect("fixture cache mutex is healthy")
            .folder_write_sync_required = true;
        ready(Ok(()))
    }

    fn clear_folder_write_sync_requirement(
        &self,
        _: &FolderReference,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
        self.0
            .lock()
            .expect("fixture cache mutex is healthy")
            .folder_write_sync_required = false;
        ready(Ok(()))
    }

    fn set_message_follow_up_flag_status(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
        status: FollowUpFlagStatus,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
        let folder = folder.clone();
        let message_id = message_id.clone();
        async move {
            let mut data = self.0.lock().expect("fixture cache mutex is healthy");
            if let Some(error) = data.read_state_write_error {
                return Err(error);
            }
            let Some(message) = data
                .messages
                .iter_mut()
                .find(|message| message.folder() == &folder && message.id() == &message_id)
            else {
                return Err(MailCacheError::MessageNotCached);
            };
            let mut patch = MessageMetadataPatch::new();
            patch.set_follow_up_flag_status(Some(status));
            message.apply_patch(patch);
            data.folder_write_sync_required = false;
            Ok(())
        }
    }

    fn move_message(
        &self,
        source_folder: &FolderReference,
        destination_folder: &FolderReference,
        message_id: &MessageId,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
        let mut data = self.0.lock().expect("fixture cache mutex is healthy");
        if let Some(error) = data.move_error {
            return ready(Err(error));
        }
        if data
            .account_id
            .as_ref()
            .is_some_and(|saved| saved != source_folder.account_id())
            || source_folder.account_id() != destination_folder.account_id()
        {
            return ready(Err(MailCacheError::AccountMismatch));
        }
        if source_folder == destination_folder
            || !data
                .folders
                .iter()
                .any(|folder| folder.reference() == destination_folder)
        {
            return ready(Err(MailCacheError::InvalidData));
        }
        let Some(index) = data
            .messages
            .iter()
            .position(|message| message.folder() == source_folder && message.id() == message_id)
        else {
            return ready(Err(MailCacheError::MessageNotCached));
        };
        let source = data.messages.remove(index);
        data.messages.push(MailMessage::new(
            destination_folder.clone(),
            source.id().clone(),
            source.metadata().clone(),
        ));
        ready(Ok(()))
    }

    fn store_message_body(
        &self,
        _: &FolderReference,
        _: &MessageId,
        body: MessageBody,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
        self.0.lock().expect("fixture cache mutex is healthy").body = Some(body);
        ready(Ok(()))
    }

    fn load_pending_send(
        &self,
        _: &AccountId,
    ) -> impl Future<Output = Result<Option<PendingMailSend>, MailCacheError>> + Send + '_ {
        ready(Ok(self
            .0
            .lock()
            .expect("fixture cache mutex is healthy")
            .pending_send
            .clone()))
    }

    fn stage_pending_send(
        &self,
        pending: &PendingMailSend,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
        let mut data = self.0.lock().expect("fixture cache mutex is healthy");
        if data.fail_next_pending_send_stage {
            data.fail_next_pending_send_stage = false;
            return ready(Err(MailCacheError::Unavailable));
        }
        if data.pending_send.is_some() {
            return ready(Err(MailCacheError::SendAlreadyPending));
        }
        data.pending_send = Some(pending.clone());
        ready(Ok(()))
    }

    fn clear_pending_send(
        &self,
        _: &AccountId,
        operation_id: MailSendOperationId,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
        let mut data = self.0.lock().expect("fixture cache mutex is healthy");
        if data.fail_next_pending_send_clear {
            data.fail_next_pending_send_clear = false;
            return ready(Err(MailCacheError::Unavailable));
        }
        match data
            .pending_send
            .as_ref()
            .map(PendingMailSend::operation_id)
        {
            Some(saved) if saved == operation_id => data.pending_send = None,
            Some(_) => return ready(Err(MailCacheError::PendingSendMismatch)),
            None => return ready(Err(MailCacheError::PendingSendNotFound)),
        }
        ready(Ok(()))
    }
}

impl AccountCache for FixtureCache {
    fn remove_account(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<(), AccountCacheRemovalError>> + Send + '_ {
        let mut data = self.0.lock().expect("fixture cache mutex is healthy");
        let result = match data.account_id.as_ref() {
            Some(saved) if saved != account_id => Err(AccountCacheRemovalError::AccountMismatch),
            _ => {
                *data = FixtureData::default();
                Ok(())
            }
        };
        ready(result)
    }
}

#[cfg(test)]
pub(crate) fn fixture(profile_account: AccountId) -> (AccountProfile, FixtureCache, MessageId) {
    let folder_reference = FolderReference::new(
        profile_account.clone(),
        FolderId::try_from("inbox".to_owned()).expect("synthetic folder ID is valid"),
    );
    let folder = MailFolder::new(folder_reference.clone(), None, "Inbox".to_owned())
        .expect("synthetic folder is valid");
    let archive_reference = FolderReference::new(
        profile_account.clone(),
        FolderId::try_from("archive".to_owned()).expect("synthetic folder ID is valid"),
    );
    let archive = MailFolder::new(archive_reference, None, "Archive".to_owned())
        .expect("synthetic archive folder is valid");
    let message_id =
        MessageId::try_from("message-1".to_owned()).expect("synthetic message ID is valid");
    let sender = Mailbox::try_new(
        "ada@example.test".to_owned(),
        Some("Ada Example".to_owned()),
    )
    .expect("synthetic mailbox is valid");
    let message = MailMessage::new(
        folder_reference,
        message_id.clone(),
        einfach_mail_domain::MessageMetadata::new(
            Some("Synthetic subject".to_owned()),
            einfach_mail_domain::MessageOrigin::new(Some(sender), None),
            UNIX_EPOCH + Duration::from_secs(1_800_000_000),
            einfach_mail_domain::ReadState::Unread,
            None,
        ),
    );
    let cache = FixtureCache::default();
    {
        let mut data = cache.0.lock().expect("fixture cache mutex is healthy");
        data.account_id = Some(profile_account.clone());
        data.folders.push(folder);
        data.folders.push(archive);
        data.messages.push(message);
        data.body = Some(MessageBody::new(
            MessageBodyType::Text,
            "Synthetic offline body".to_owned(),
        ));
        data.sync_state = Some(
            MailSyncState::from_checkpoint(
                UNIX_EPOCH + Duration::from_secs(1_700_000_000),
                Some(MessageDeltaLink::from_opaque(
                    "https://graph.microsoft.com/v1.0/delta?token=synthetic".to_owned(),
                )),
                None,
                Some(UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
                None,
                0,
            )
            .expect("synthetic sync checkpoint is valid"),
        );
    }
    let profile = synthetic_account_profile(
        profile_account,
        Some("ada@tenant.example.test".to_owned()),
        AccountCapabilities::full_access(),
    )
    .expect("synthetic account profile is valid");
    (profile, cache, message_id)
}
