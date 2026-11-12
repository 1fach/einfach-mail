use std::{sync::Arc, time::SystemTime};

use crate::components::relative_age_label;
use einfach_mail_application::{
    MailCacheError, MailSession, MailSyncProgress, MailSyncReport, MailSyncState, MailWorkflowError,
};
use einfach_mail_domain::{FolderReference, MailFolder, MailMessage, MessageId, PendingMailSend};
use gpui_kit::{AsyncApp, Context};

use super::{
    FollowUpFlagUpdateStatus, MailView, MessageDetailState, MessageListState, MessageMoveStatus,
    ReadStateUpdateStatus, WellKnownFolderActionStatus, load_folder_snapshot, mail_error_message,
};

pub(super) enum SyncStatus {
    Idle,
    LoadingCache,
    Syncing,
    Rebuilding,
    UpToDate,
    Failed(MailWorkflowError),
}

impl SyncStatus {
    pub(super) fn is_sync_active(&self) -> bool {
        match self {
            Self::LoadingCache | Self::Syncing | Self::Rebuilding => true,
            Self::Idle | Self::UpToDate | Self::Failed(_) => false,
        }
    }
}

impl MailView {
    pub(super) fn load_cache_and_sync(&mut self, cx: &mut Context<Self>) {
        let workflow = Arc::clone(&self.workflow);
        self.sync_task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let initial_snapshot = load_snapshot(workflow.as_ref(), None).await;
            let has_snapshot = initial_snapshot.is_ok();
            let _ = this.update(cx, |view, cx| match initial_snapshot {
                Ok(snapshot) => {
                    view.apply_snapshot(snapshot);
                    view.sync_status = SyncStatus::Syncing;
                    cx.notify();
                }
                Err(error) => {
                    view.sync_status = SyncStatus::Failed(error);
                    view.message_list_state = MessageListState::Failed(error);
                    cx.notify();
                }
            });
            if !has_snapshot {
                return;
            }

            let sync_result =
                synchronize_with_progress(workflow.as_ref(), SystemTime::now(), |progress| {
                    let _ = this.update(cx, |view, cx| {
                        view.apply_sync_progress(progress);
                        cx.notify();
                    });
                })
                .await;
            let Ok(selected_folder) = this.read_with(cx, |view, _| view.selected_folder.clone())
            else {
                return;
            };
            let snapshot = load_snapshot(workflow.as_ref(), selected_folder.as_ref()).await;
            let _ = this.update(cx, |view, cx| {
                view.sync_task = None;
                if let Some(message_id) =
                    view.finish_sync(sync_result.map(|_| ()), snapshot, selected_folder)
                {
                    view.select_message(message_id, cx);
                }
                cx.notify();
            });
        }));
    }

    fn apply_snapshot(&mut self, snapshot: MailSnapshot) {
        self.folders = snapshot.folders;
        self.selected_folder = snapshot.selected_folder;
        self.set_messages(snapshot.messages);
        self.sync_state = snapshot.sync_state;
        self.selected_folder_write_sync_required = snapshot.folder_write_sync_required;
        self.pending_send = snapshot.pending_send;
        self.message_list_state = MessageListState::Ready;
        self.message_detail_state = MessageDetailState::Empty;
        self.message_task = None;
        self.read_state_update = ReadStateUpdateStatus::Idle;
        self.follow_up_flag_update = FollowUpFlagUpdateStatus::Idle;
        self.message_move_status = MessageMoveStatus::Idle;
        self.well_known_folder_action_status = WellKnownFolderActionStatus::Idle;
    }

    pub(super) fn apply_sync_progress(&mut self, progress: MailSyncProgress) {
        match progress {
            MailSyncProgress::Rebuilding => self.sync_status = SyncStatus::Rebuilding,
        }
    }

    pub(super) fn finish_sync(
        &mut self,
        sync_result: Result<(), MailWorkflowError>,
        snapshot: Result<MailSnapshot, MailWorkflowError>,
        requested_folder: Option<FolderReference>,
    ) -> Option<MessageId> {
        let sync_reconciled = sync_result.is_ok() && snapshot.is_ok();
        let (status, snapshot) = match (sync_result, snapshot) {
            (_, Err(error)) => (SyncStatus::Failed(error), None),
            (Err(error), Ok(snapshot)) => (SyncStatus::Failed(error), Some(snapshot)),
            (Ok(_), Ok(snapshot)) => (SyncStatus::UpToDate, Some(snapshot)),
        };
        let snapshot =
            snapshot.filter(|_| self.selected_folder.as_ref() == requested_folder.as_ref());
        let message_to_reopen = snapshot.as_ref().and_then(|snapshot| {
            let selected_message = self.selected_message_id()?;
            snapshot
                .messages
                .iter()
                .any(|message| message.id() == selected_message)
                .then(|| selected_message.clone())
        });
        if let Some(snapshot) = snapshot {
            self.apply_snapshot(snapshot);
            if let Some(message_id) = &message_to_reopen {
                self.message_detail_state = MessageDetailState::Loading(message_id.clone());
            }
        }
        if sync_reconciled {
            self.move_recovery_reason = None;
            self.message_move_status = MessageMoveStatus::Idle;
            self.well_known_folder_action_status = WellKnownFolderActionStatus::Idle;
        }
        self.sync_status = status;
        message_to_reopen
    }

    pub(super) fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.sync_status.is_sync_active() || self.message_update_in_progress() {
            return;
        }
        self.sync_status = SyncStatus::Syncing;
        let workflow = Arc::clone(&self.workflow);
        self.sync_task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let sync_result =
                synchronize_with_progress(workflow.as_ref(), SystemTime::now(), |progress| {
                    let _ = this.update(cx, |view, cx| {
                        view.apply_sync_progress(progress);
                        cx.notify();
                    });
                })
                .await;
            let Ok(selected_folder) = this.read_with(cx, |view, _| view.selected_folder.clone())
            else {
                return;
            };
            let snapshot = load_snapshot(workflow.as_ref(), selected_folder.as_ref()).await;
            let _ = this.update(cx, |view, cx| {
                view.sync_task = None;
                if let Some(message_id) =
                    view.finish_sync(sync_result.map(|_| ()), snapshot, selected_folder)
                {
                    view.select_message(message_id, cx);
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn sync_status_label(&self) -> String {
        match &self.sync_status {
            SyncStatus::Idle => "Mail is ready to sync".to_owned(),
            SyncStatus::LoadingCache => "Opening encrypted cache…".to_owned(),
            SyncStatus::Syncing => {
                let label = if self
                    .sync_state
                    .as_ref()
                    .is_some_and(MailSyncState::is_rebuilding)
                {
                    "Rebuilding mail"
                } else {
                    "Syncing"
                };
                self.sync_age_label(label)
            }
            SyncStatus::Rebuilding => self.sync_age_label("Rebuilding mail"),
            SyncStatus::UpToDate => self.sync_age_label("Up to date"),
            SyncStatus::Failed(error) => {
                let rebuild_resume = self
                    .sync_state
                    .as_ref()
                    .is_some_and(MailSyncState::is_rebuilding)
                    && *error != MailWorkflowError::RepeatedDeltaReset;
                let cached_age = self.sync_state.as_ref().and_then(|state| {
                    state.last_synchronized_at().map(|last_sync| {
                        format!(" Cached mail from {}.", relative_age_label(last_sync))
                    })
                });
                format!(
                    "{}{}{}",
                    mail_error_message(*error),
                    cached_age.unwrap_or_default(),
                    if rebuild_resume {
                        " The saved mail rebuild is incomplete. Choose Sync to resume it."
                    } else {
                        ""
                    }
                )
            }
        }
    }

    fn sync_age_label(&self, prefix: &str) -> String {
        self.sync_state
            .as_ref()
            .and_then(MailSyncState::last_synchronized_at)
            .map_or_else(
                || prefix.to_owned(),
                |time| format!("{prefix} · {}", relative_age_label(time)),
            )
    }
}

pub(super) struct MailSnapshot {
    folders: Vec<MailFolder>,
    selected_folder: Option<FolderReference>,
    messages: Vec<MailMessage>,
    sync_state: Option<MailSyncState>,
    folder_write_sync_required: bool,
    pending_send: Result<Option<PendingMailSend>, MailCacheError>,
}

async fn load_snapshot(
    workflow: &dyn MailSession,
    preferred: Option<&FolderReference>,
) -> Result<MailSnapshot, MailWorkflowError> {
    let folders = workflow.list_cached_folders().await?;
    let pending_send = workflow.load_pending_send().await;
    let selected_folder = preferred
        .filter(|preferred| {
            folders
                .iter()
                .any(|folder| folder.reference() == *preferred)
        })
        .cloned()
        .or_else(|| preferred_folder(&folders));
    let (messages, sync_state, folder_write_sync_required) = if let Some(folder) = &selected_folder
    {
        let snapshot = load_folder_snapshot(workflow, folder).await?;
        (
            snapshot.messages,
            snapshot.sync_state,
            snapshot.folder_write_sync_required,
        )
    } else {
        (Vec::new(), None, false)
    };
    Ok(MailSnapshot {
        folders,
        selected_folder,
        messages,
        sync_state,
        folder_write_sync_required,
        pending_send,
    })
}

fn preferred_folder(folders: &[MailFolder]) -> Option<FolderReference> {
    folders
        .iter()
        .find(|folder| folder.display_name().eq_ignore_ascii_case("inbox"))
        .or_else(|| folders.first())
        .map(|folder| folder.reference().clone())
}

fn mail_sync_progress_channel() -> (
    impl Fn(MailSyncProgress) + Send + Sync + 'static,
    tokio::sync::watch::Receiver<Option<MailSyncProgress>>,
) {
    let (sender, receiver) = tokio::sync::watch::channel(None);
    let progress_handler = move |progress| {
        sender.send_replace(Some(progress));
    };
    (progress_handler, receiver)
}

async fn synchronize_with_progress(
    workflow: &dyn MailSession,
    sync_started_at: SystemTime,
    mut on_progress: impl FnMut(MailSyncProgress),
) -> Result<MailSyncReport, MailWorkflowError> {
    let (progress_handler, mut progress_receiver) = mail_sync_progress_channel();
    let mut sync_future =
        Box::pin(workflow.synchronize_account(sync_started_at, &progress_handler));
    loop {
        let changed = tokio::select! {
            biased;
            changed = progress_receiver.changed() => changed,
            sync_result = &mut sync_future => return sync_result,
        };
        if changed.is_err() {
            return sync_future.await;
        }
        if let Some(progress) = *progress_receiver.borrow_and_update() {
            on_progress(progress);
        }
    }
}
