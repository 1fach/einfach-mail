use std::sync::Arc;

use einfach_mail_application::{FollowUpFlagUpdateOutcome, MailOperationError, MailWorkflowError};
use einfach_mail_domain::{FollowUpFlagStatus, MailAction, MessageId, MessageMetadataPatch};
use gpui_kit::{AsyncApp, Context};

use super::MailView;
use super::message_write::{MessageWriteFeedbackText, MessageWriteRequest, MessageWriteStatus};

pub(super) type FollowUpFlagUpdateStatus = MessageWriteStatus<FollowUpFlagStatus>;
pub(super) type FollowUpFlagUpdateRequest = MessageWriteRequest<FollowUpFlagStatus>;

impl MessageWriteStatus<FollowUpFlagStatus> {
    pub(super) fn action_label(
        &self,
        message_id: &MessageId,
        flag_status: Option<FollowUpFlagStatus>,
    ) -> &'static str {
        if self.is_pending_for(message_id) {
            return "Updating…";
        }
        match flag_status {
            Some(FollowUpFlagStatus::Flagged) => "Clear flag",
            Some(FollowUpFlagStatus::Complete) => "Reopen flag",
            Some(FollowUpFlagStatus::NotFlagged) | None => "Flag",
        }
    }
}

impl MessageWriteFeedbackText for FollowUpFlagStatus {
    fn progress_message(self) -> String {
        follow_up_flag_progress_message(self)
    }

    fn confirmed_message(self) -> &'static str {
        follow_up_flag_confirmed_message(self)
    }

    fn cache_sync_message(self) -> &'static str {
        follow_up_flag_cache_sync_message(self)
    }

    fn failure_message(self, error: MailWorkflowError) -> String {
        follow_up_flag_failure_message(self, error)
    }
}

impl MailView {
    pub(super) fn update_selected_follow_up_flag(&mut self, cx: &mut Context<Self>) {
        if self.sync_status.is_sync_active() || self.message_update_in_progress() {
            return;
        }
        let Some(request) = self.selected_follow_up_flag_request() else {
            return;
        };
        let workflow = Arc::clone(&self.workflow);
        self.follow_up_flag_update = FollowUpFlagUpdateStatus::Pending {
            message_id: request.message_id.clone(),
            requested: request.requested,
        };
        self.message_update_task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let flag_update_outcome = workflow
                .set_message_follow_up_flag_status(
                    &request.folder,
                    &request.message_id,
                    request.requested,
                )
                .await;
            let write_sync_required = workflow
                .folder_write_sync_required(&request.folder)
                .await
                .unwrap_or(true);
            let _ = this.update(cx, |view, cx| {
                view.finish_follow_up_flag_update(
                    request,
                    flag_update_outcome,
                    write_sync_required,
                );
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn selected_follow_up_flag_request(&self) -> Option<FollowUpFlagUpdateRequest> {
        let folder = self.selected_folder.as_ref()?;
        if !self.can_update_message_in(folder, MailAction::SetFollowUpFlagStatus) {
            return None;
        }
        let folder = folder.clone();
        let message_id = self.selected_message_id()?.clone();
        let current_status = self
            .messages
            .iter()
            .find(|message| message.id() == &message_id)?
            .metadata()
            .follow_up_flag_status();
        let requested = match current_status {
            Some(FollowUpFlagStatus::Flagged) => FollowUpFlagStatus::NotFlagged,
            Some(FollowUpFlagStatus::NotFlagged | FollowUpFlagStatus::Complete) | None => {
                FollowUpFlagStatus::Flagged
            }
        };
        Some(FollowUpFlagUpdateRequest {
            folder,
            message_id,
            requested,
        })
    }

    fn finish_follow_up_flag_update(
        &mut self,
        request: FollowUpFlagUpdateRequest,
        flag_update_outcome: Result<FollowUpFlagUpdateOutcome, MailWorkflowError>,
        write_sync_required: bool,
    ) {
        self.follow_up_flag_update = if self.selected_folder.as_ref() == Some(&request.folder) {
            self.selected_folder_write_sync_required = write_sync_required;
            self.apply_follow_up_flag_result(request, flag_update_outcome)
        } else {
            FollowUpFlagUpdateStatus::Idle
        };
        self.message_update_task = None;
    }

    fn apply_follow_up_flag_result(
        &mut self,
        request: FollowUpFlagUpdateRequest,
        flag_update_outcome: Result<FollowUpFlagUpdateOutcome, MailWorkflowError>,
    ) -> FollowUpFlagUpdateStatus {
        match flag_update_outcome {
            Ok(FollowUpFlagUpdateOutcome::ConfirmedAndCached(status)) => {
                self.apply_follow_up_flag_to_message(&request.message_id, status);
                FollowUpFlagUpdateStatus::Confirmed {
                    message_id: request.message_id,
                    status,
                }
            }
            Ok(FollowUpFlagUpdateOutcome::ConfirmedNeedsSync { status, .. }) => {
                self.apply_follow_up_flag_to_message(&request.message_id, status);
                FollowUpFlagUpdateStatus::ConfirmedNeedsSync {
                    message_id: request.message_id,
                    status,
                }
            }
            Err(error) => FollowUpFlagUpdateStatus::Failed {
                message_id: request.message_id,
                requested: request.requested,
                error,
            },
        }
    }

    fn apply_follow_up_flag_to_message(
        &mut self,
        message_id: &MessageId,
        status: FollowUpFlagStatus,
    ) {
        let Some(message) = self
            .messages
            .iter_mut()
            .find(|message| message.id() == message_id)
        else {
            return;
        };
        let mut patch = MessageMetadataPatch::new();
        patch.set_follow_up_flag_status(Some(status));
        message.apply_patch(patch);
    }
}

pub(super) fn follow_up_flag_progress_message(status: FollowUpFlagStatus) -> String {
    match status {
        FollowUpFlagStatus::NotFlagged => "Clearing the follow-up flag…".to_owned(),
        FollowUpFlagStatus::Flagged => "Flagging the message for follow-up…".to_owned(),
        FollowUpFlagStatus::Complete => "Marking follow-up complete…".to_owned(),
    }
}

pub(super) fn follow_up_flag_confirmed_message(status: FollowUpFlagStatus) -> &'static str {
    match status {
        FollowUpFlagStatus::NotFlagged => "Follow-up flag cleared",
        FollowUpFlagStatus::Flagged => "Flagged for follow-up",
        FollowUpFlagStatus::Complete => "Follow-up marked complete",
    }
}

pub(super) fn follow_up_flag_cache_sync_message(status: FollowUpFlagStatus) -> &'static str {
    match status {
        FollowUpFlagStatus::NotFlagged => {
            "Microsoft cleared the follow-up flag. Sync this folder to update the encrypted cache."
        }
        FollowUpFlagStatus::Flagged => {
            "Microsoft flagged this message. Sync this folder to update the encrypted cache."
        }
        FollowUpFlagStatus::Complete => {
            "Microsoft marked follow-up complete. Sync this folder to update the encrypted cache."
        }
    }
}

pub(super) fn follow_up_flag_failure_message(
    status: FollowUpFlagStatus,
    error: MailWorkflowError,
) -> String {
    match error {
        MailWorkflowError::Provider(MailOperationError::AccessDenied) => {
            "Microsoft denied the flag change. Review mail permissions or check your organization's consent policies.".to_owned()
        }
        MailWorkflowError::Provider(MailOperationError::SignInRequired) => {
            "Your Microsoft sign-in needs attention. Return to Accounts and reconnect.".to_owned()
        }
        MailWorkflowError::Provider(MailOperationError::RateLimited { .. }) => {
            "Microsoft asked Einfach Mail to wait before retrying. Try again later.".to_owned()
        }
        MailWorkflowError::Provider(MailOperationError::MessageUpdateOutcomeUnknown) => {
            "Microsoft may have changed this message's follow-up flag, but Einfach Mail could not confirm it. Sync this folder before trying again.".to_owned()
        }
        MailWorkflowError::FolderSyncRequired => {
            "Sync this folder before changing message state again.".to_owned()
        }
        MailWorkflowError::Provider(MailOperationError::ServiceUnavailable) => {
            "Couldn’t update the follow-up flag. Check your connection and try again.".to_owned()
        }
        _ => format!(
            "Couldn’t {} the follow-up flag. Sync this folder and try again.",
            match status {
                FollowUpFlagStatus::NotFlagged => "clear",
                FollowUpFlagStatus::Flagged => "set",
                FollowUpFlagStatus::Complete => "complete",
            }
        ),
    }
}
