use std::sync::Arc;

use einfach_mail_application::{MailOperationError, MailWorkflowError, ReadStateUpdateOutcome};
use einfach_mail_domain::{
    FolderReference, MailAction, MessageId, MessageMetadataPatch, ReadState,
};
use gpui_kit::{AsyncApp, Context, Role};

use super::MailView;

pub(super) enum ReadStateUpdateStatus {
    Idle,
    Pending {
        message_id: MessageId,
        requested: ReadState,
    },
    Confirmed {
        message_id: MessageId,
        state: ReadState,
    },
    ConfirmedNeedsSync {
        message_id: MessageId,
        state: ReadState,
    },
    Failed {
        message_id: MessageId,
        requested: ReadState,
        error: MailWorkflowError,
    },
}

pub(super) enum ReadStateFeedback {
    Status(String),
    Failure {
        message: String,
        error: MailWorkflowError,
    },
}

impl ReadStateFeedback {
    pub(super) fn into_parts(self) -> (String, Role, bool) {
        match self {
            Self::Status(message) => (message, Role::Status, false),
            Self::Failure { message, error } => (
                message,
                Role::Alert,
                matches!(
                    error,
                    MailWorkflowError::Provider(MailOperationError::AccessDenied)
                ),
            ),
        }
    }
}

impl ReadStateUpdateStatus {
    pub(super) fn is_pending_for(&self, message_id: &MessageId) -> bool {
        matches!(
            self,
            Self::Pending {
                message_id: pending_id,
                ..
            } if pending_id == message_id
        )
    }

    pub(super) fn feedback_for(&self, message_id: &MessageId) -> Option<ReadStateFeedback> {
        if self.message_id()? != message_id {
            return None;
        }
        self.feedback()
    }

    fn message_id(&self) -> Option<&MessageId> {
        match self {
            Self::Idle => None,
            Self::Pending { message_id, .. }
            | Self::Confirmed { message_id, .. }
            | Self::ConfirmedNeedsSync { message_id, .. }
            | Self::Failed { message_id, .. } => Some(message_id),
        }
    }

    fn feedback(&self) -> Option<ReadStateFeedback> {
        match self {
            Self::Idle => None,
            Self::Pending { requested, .. } => Some(ReadStateFeedback::Status(
                read_state_progress_message(*requested),
            )),
            Self::Confirmed { state, .. } => Some(ReadStateFeedback::Status(
                read_state_confirmed_message(*state).to_owned(),
            )),
            Self::ConfirmedNeedsSync { state, .. } => Some(ReadStateFeedback::Status(
                read_state_cache_sync_message(*state).to_owned(),
            )),
            Self::Failed {
                requested, error, ..
            } => Some(ReadStateFeedback::Failure {
                message: read_state_failure_message(*requested, *error),
                error: *error,
            }),
        }
    }
}

pub(super) struct ReadStateUpdateRequest {
    folder: FolderReference,
    message_id: MessageId,
    requested: ReadState,
}

impl MailView {
    pub(super) fn update_selected_read_state(&mut self, cx: &mut Context<Self>) {
        if self.sync_status.is_sync_active() || self.message_update_in_progress() {
            return;
        }
        let Some(request) = self.selected_read_state_request() else {
            return;
        };
        let workflow = Arc::clone(&self.workflow);
        self.read_state_update = ReadStateUpdateStatus::Pending {
            message_id: request.message_id.clone(),
            requested: request.requested,
        };
        self.message_update_task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let update_outcome = workflow
                .set_message_read_state(&request.folder, &request.message_id, request.requested)
                .await;
            let write_sync_required = workflow
                .folder_write_sync_required(&request.folder)
                .await
                .unwrap_or(true);
            let _ = this.update(cx, |view, cx| {
                view.finish_read_state_update(request, update_outcome, write_sync_required);
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn selected_read_state_request(&self) -> Option<ReadStateUpdateRequest> {
        let folder = self.selected_folder.as_ref()?;
        if !self.can_update_message_in(folder, MailAction::SetReadState) {
            return None;
        }
        let folder = folder.clone();
        let message_id = self.selected_message_id()?.clone();
        let current_state = self
            .messages
            .iter()
            .find(|message| message.id() == &message_id)?
            .metadata()
            .read_state();
        let requested = match current_state {
            ReadState::Read => ReadState::Unread,
            ReadState::Unread => ReadState::Read,
        };
        Some(ReadStateUpdateRequest {
            folder,
            message_id,
            requested,
        })
    }

    fn finish_read_state_update(
        &mut self,
        request: ReadStateUpdateRequest,
        update_outcome: Result<ReadStateUpdateOutcome, MailWorkflowError>,
        write_sync_required: bool,
    ) {
        self.read_state_update = if self.selected_folder.as_ref() == Some(&request.folder) {
            self.selected_folder_write_sync_required = write_sync_required;
            self.apply_read_state_result(request, update_outcome)
        } else {
            ReadStateUpdateStatus::Idle
        };
        self.message_update_task = None;
    }

    fn apply_read_state_result(
        &mut self,
        request: ReadStateUpdateRequest,
        update_outcome: Result<ReadStateUpdateOutcome, MailWorkflowError>,
    ) -> ReadStateUpdateStatus {
        match update_outcome {
            Ok(ReadStateUpdateOutcome::ConfirmedAndCached(state)) => {
                self.apply_read_state_to_message(&request.message_id, state);
                ReadStateUpdateStatus::Confirmed {
                    message_id: request.message_id,
                    state,
                }
            }
            Ok(ReadStateUpdateOutcome::ConfirmedNeedsSync { state, .. }) => {
                self.apply_read_state_to_message(&request.message_id, state);
                ReadStateUpdateStatus::ConfirmedNeedsSync {
                    message_id: request.message_id,
                    state,
                }
            }
            Err(error) => ReadStateUpdateStatus::Failed {
                message_id: request.message_id,
                requested: request.requested,
                error,
            },
        }
    }

    fn apply_read_state_to_message(&mut self, message_id: &MessageId, state: ReadState) {
        let Some(message) = self
            .messages
            .iter_mut()
            .find(|message| message.id() == message_id)
        else {
            return;
        };
        let mut patch = MessageMetadataPatch::new();
        patch.set_read_state(state);
        message.apply_patch(patch);
    }
}

fn read_state_progress_message(state: ReadState) -> String {
    match state {
        ReadState::Read => "Marking the message as read…".to_owned(),
        ReadState::Unread => "Marking the message as unread…".to_owned(),
    }
}

fn read_state_confirmed_message(state: ReadState) -> &'static str {
    match state {
        ReadState::Read => "Marked read",
        ReadState::Unread => "Marked unread",
    }
}

fn read_state_cache_sync_message(state: ReadState) -> &'static str {
    match state {
        ReadState::Read => {
            "Microsoft marked this message read. Sync mail to update the encrypted cache."
        }
        ReadState::Unread => {
            "Microsoft marked this message unread. Sync mail to update the encrypted cache."
        }
    }
}

pub(super) fn read_state_failure_message(state: ReadState, error: MailWorkflowError) -> String {
    match error {
        MailWorkflowError::Provider(MailOperationError::ServiceUnavailable) => format!(
            "Couldn’t mark the message {}. Check your connection and try again.",
            read_state_word(state)
        ),
        _ => read_state_failure_detail(error).map_or_else(
            || {
                format!(
                    "Couldn’t mark the message {}. Sync mail and try again.",
                    read_state_word(state)
                )
            },
            str::to_owned,
        ),
    }
}

fn read_state_failure_detail(error: MailWorkflowError) -> Option<&'static str> {
    match error {
        MailWorkflowError::Provider(MailOperationError::AccessDenied) => Some(
            "Microsoft denied the change. Review mail permissions or check your organization's consent policies.",
        ),
        MailWorkflowError::Provider(MailOperationError::SignInRequired) => {
            Some("Your Microsoft sign-in needs attention. Return to Accounts and reconnect.")
        }
        MailWorkflowError::Provider(MailOperationError::RateLimited { .. }) => {
            Some("Microsoft asked Einfach Mail to wait before retrying. Try again later.")
        }
        MailWorkflowError::Provider(MailOperationError::MessageUpdateOutcomeUnknown) => Some(
            "Microsoft may have changed this message's read state, but Einfach Mail could not confirm it. Sync mail before trying again.",
        ),
        _ => None,
    }
}

fn read_state_word(state: ReadState) -> &'static str {
    match state {
        ReadState::Read => "read",
        ReadState::Unread => "unread",
    }
}
