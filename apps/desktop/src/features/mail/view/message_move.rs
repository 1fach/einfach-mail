use std::sync::Arc;

use einfach_mail_application::{
    MailOperationError, MailWorkflowError, MessageMoveOutcome, WellKnownFolderMoveOutcome,
};
use einfach_mail_domain::{FolderReference, MailAction, MailFolderKind, MessageId};
use gpui_kit::{AsyncApp, Context};

use super::{MailView, MessageDetailState, SyncStatus};

pub(super) enum MessageMoveStatus {
    Idle,
    Pending {
        message_id: MessageId,
        destination_label: String,
    },
    Failed {
        message_id: MessageId,
        error: MailWorkflowError,
    },
}

pub(super) enum WellKnownFolderActionStatus {
    Idle,
    Pending {
        message_id: MessageId,
        destination: MailFolderKind,
    },
    Failed {
        message_id: MessageId,
        destination: MailFolderKind,
        error: MailWorkflowError,
    },
}

#[derive(Clone, Copy)]
pub(super) enum MoveRecoveryReason {
    InitialSyncRequired,
    OutcomeUnknown,
    CacheNeedsSync,
}

pub(super) struct MessageMoveRequest {
    source_folder: FolderReference,
    message_id: MessageId,
    destination_folder: FolderReference,
    destination_label: String,
}

pub(super) struct WellKnownFolderActionRequest {
    source_folder: FolderReference,
    message_id: MessageId,
    destination: MailFolderKind,
}

#[derive(Clone)]
pub(super) struct MessageMoveMenuAction {
    pub(super) selected_message_id: MessageId,
    pub(super) destination_folder: FolderReference,
    pub(super) destination_label: String,
}

pub(super) fn well_known_folder_mail_action(destination: MailFolderKind) -> MailAction {
    match destination {
        MailFolderKind::DeletedItems => MailAction::Delete,
        MailFolderKind::Inbox
        | MailFolderKind::SentItems
        | MailFolderKind::Drafts
        | MailFolderKind::Archive
        | MailFolderKind::JunkEmail => MailAction::Move,
    }
}

impl MailView {
    pub(super) fn move_selected_message_to(
        &mut self,
        destination_folder: FolderReference,
        destination_label: String,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = self.prepare_message_move(destination_folder, destination_label) else {
            return;
        };
        self.start_message_move(request, cx);
    }

    pub(super) fn apply_message_move_menu_action(
        &mut self,
        action: MessageMoveMenuAction,
        cx: &mut Context<Self>,
    ) {
        if self.selected_message_id() != Some(&action.selected_message_id) {
            return;
        }
        self.move_selected_message_to(action.destination_folder, action.destination_label, cx);
    }

    pub(super) fn prepare_message_move(
        &self,
        destination_folder: FolderReference,
        destination_label: String,
    ) -> Option<MessageMoveRequest> {
        if !self.message_move_allowed(MailAction::Move) {
            return None;
        }
        let source_folder = self.selected_folder.clone()?;
        let message_id = self.selected_message_id()?.clone();
        if source_folder == destination_folder
            || source_folder.account_id() != destination_folder.account_id()
        {
            return None;
        }
        Some(MessageMoveRequest {
            source_folder,
            message_id,
            destination_folder,
            destination_label,
        })
    }

    pub(super) fn start_well_known_folder_action(
        &mut self,
        destination: MailFolderKind,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = self.prepare_well_known_folder_action(destination) else {
            return;
        };
        self.well_known_folder_action_status = WellKnownFolderActionStatus::Pending {
            message_id: request.message_id.clone(),
            destination,
        };
        self.spawn_well_known_folder_action(request, cx);
    }

    fn spawn_well_known_folder_action(
        &mut self,
        request: WellKnownFolderActionRequest,
        cx: &mut Context<Self>,
    ) {
        let workflow = Arc::clone(&self.workflow);
        self.message_update_task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let action_result = workflow
                .move_message_to_well_known_folder(
                    &request.source_folder,
                    &request.message_id,
                    request.destination,
                )
                .await;
            let _ = this.update(cx, |view, cx| {
                view.finish_well_known_folder_action(request, action_result);
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn prepare_well_known_folder_action(
        &self,
        destination: MailFolderKind,
    ) -> Option<WellKnownFolderActionRequest> {
        if !self.message_move_allowed(well_known_folder_mail_action(destination)) {
            return None;
        }
        Some(WellKnownFolderActionRequest {
            source_folder: self.selected_folder.clone()?,
            message_id: self.selected_message_id()?.clone(),
            destination,
        })
    }

    pub(super) fn message_move_allowed(&self, action: MailAction) -> bool {
        self.profile.capabilities().mail().supports(action)
            && matches!(&self.sync_status, SyncStatus::UpToDate)
            && !self.selected_folder_write_sync_required
            && !self.message_update_in_progress()
            && self.move_recovery_reason.is_none()
    }

    fn start_message_move(&mut self, request: MessageMoveRequest, cx: &mut Context<Self>) {
        let workflow = Arc::clone(&self.workflow);
        self.message_move_status = MessageMoveStatus::Pending {
            message_id: request.message_id.clone(),
            destination_label: request.destination_label.clone(),
        };
        self.message_update_task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let move_result = workflow
                .move_message(
                    &request.source_folder,
                    &request.message_id,
                    &request.destination_folder,
                )
                .await;
            let _ = this.update(cx, |view, cx| {
                view.finish_message_move(request, move_result);
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn finish_message_move(
        &mut self,
        request: MessageMoveRequest,
        move_result: Result<MessageMoveOutcome, MailWorkflowError>,
    ) {
        self.message_update_task = None;
        match move_result {
            Ok(outcome) => self.finish_confirmed_message_move(request, outcome),
            Err(error) => self.finish_failed_message_move(request.message_id, &error),
        }
    }

    fn finish_confirmed_message_move(
        &mut self,
        request: MessageMoveRequest,
        outcome: MessageMoveOutcome,
    ) {
        match outcome {
            MessageMoveOutcome::ConfirmedAndCached => {
                self.remove_confirmed_message(&request.message_id);
                self.message_move_status = MessageMoveStatus::Idle;
                self.show_message_move_completion(
                    request.destination_folder,
                    request.destination_label,
                    "Open folder",
                );
            }
            MessageMoveOutcome::ConfirmedNeedsSync => {
                self.require_move_sync(MoveRecoveryReason::CacheNeedsSync);
            }
        }
    }

    fn show_message_move_completion(
        &mut self,
        destination_folder: FolderReference,
        destination_label: String,
        open_action_label: &'static str,
    ) {
        self.message_detail_state = MessageDetailState::MoveCompleted {
            destination_folder,
            destination_label,
            open_action_label,
        };
    }

    fn require_move_sync(&mut self, reason: MoveRecoveryReason) {
        self.message_detail_state = MessageDetailState::Empty;
        self.message_move_status = MessageMoveStatus::Idle;
        self.well_known_folder_action_status = WellKnownFolderActionStatus::Idle;
        self.move_recovery_reason = Some(reason);
    }

    fn finish_failed_message_move(&mut self, message_id: MessageId, error: &MailWorkflowError) {
        if message_move_outcome_is_unknown(error) {
            self.move_recovery_reason = Some(MoveRecoveryReason::OutcomeUnknown);
            self.message_move_status = MessageMoveStatus::Idle;
        } else {
            self.message_move_status = MessageMoveStatus::Failed {
                message_id,
                error: *error,
            };
        }
    }

    fn finish_well_known_folder_action(
        &mut self,
        request: WellKnownFolderActionRequest,
        action_result: Result<WellKnownFolderMoveOutcome, MailWorkflowError>,
    ) {
        self.message_update_task = None;
        match action_result {
            Ok(outcome) => self.finish_confirmed_well_known_folder_action(&request, &outcome),
            Err(error) => self.finish_failed_well_known_folder_action(
                request.message_id,
                request.destination,
                &error,
            ),
        }
    }

    fn finish_confirmed_well_known_folder_action(
        &mut self,
        request: &WellKnownFolderActionRequest,
        outcome: &WellKnownFolderMoveOutcome,
    ) {
        match outcome.cache_outcome() {
            MessageMoveOutcome::ConfirmedAndCached => {
                self.finish_cached_well_known_folder_action(
                    request,
                    outcome.destination_folder().clone(),
                );
            }
            MessageMoveOutcome::ConfirmedNeedsSync => {
                self.require_move_sync(MoveRecoveryReason::CacheNeedsSync);
            }
        }
    }

    fn finish_cached_well_known_folder_action(
        &mut self,
        request: &WellKnownFolderActionRequest,
        destination_folder: FolderReference,
    ) {
        self.remove_confirmed_message(&request.message_id);
        self.well_known_folder_action_status = WellKnownFolderActionStatus::Idle;
        match request.destination {
            MailFolderKind::Archive => self.show_message_move_completion(
                destination_folder,
                "Archive".to_owned(),
                "Open Archive",
            ),
            MailFolderKind::DeletedItems => {
                self.message_detail_state = MessageDetailState::DeleteCompleted;
            }
            _ => self.show_message_move_completion(
                destination_folder,
                "that folder".to_owned(),
                "Open folder",
            ),
        }
    }

    fn finish_failed_well_known_folder_action(
        &mut self,
        message_id: MessageId,
        destination: MailFolderKind,
        error: &MailWorkflowError,
    ) {
        if message_move_outcome_is_unknown(error) {
            self.move_recovery_reason = Some(MoveRecoveryReason::OutcomeUnknown);
            self.well_known_folder_action_status = WellKnownFolderActionStatus::Idle;
        } else {
            self.well_known_folder_action_status = WellKnownFolderActionStatus::Failed {
                message_id,
                destination,
                error: *error,
            };
        }
    }

    fn remove_confirmed_message(&mut self, message_id: &MessageId) {
        let messages = self
            .messages
            .drain(..)
            .filter(|message| message.id() != message_id)
            .collect();
        self.set_messages(messages);
    }

    pub(super) fn message_move_pending_for(&self, message_id: &MessageId) -> bool {
        matches!(
            &self.message_move_status,
            MessageMoveStatus::Pending {
                message_id: pending_id,
                ..
            } if *pending_id == *message_id
        )
    }

    pub(super) fn well_known_folder_action_in_progress_for(
        &self,
        message_id: &MessageId,
        destination: MailFolderKind,
    ) -> bool {
        matches!(
            &self.well_known_folder_action_status,
            WellKnownFolderActionStatus::Pending {
                message_id: pending_id,
                destination: pending_destination,
            } if pending_id == message_id && *pending_destination == destination
        )
    }

    pub(super) fn message_move_status_for(
        &self,
        message_id: &MessageId,
    ) -> Option<&MessageMoveStatus> {
        match &self.message_move_status {
            MessageMoveStatus::Pending {
                message_id: pending_id,
                ..
            }
            | MessageMoveStatus::Failed {
                message_id: pending_id,
                ..
            } if *pending_id == *message_id => Some(&self.message_move_status),
            _ => None,
        }
    }

    pub(super) fn well_known_folder_action_status_for(
        &self,
        message_id: &MessageId,
    ) -> Option<&WellKnownFolderActionStatus> {
        match &self.well_known_folder_action_status {
            WellKnownFolderActionStatus::Pending {
                message_id: pending_id,
                ..
            }
            | WellKnownFolderActionStatus::Failed {
                message_id: pending_id,
                ..
            } if pending_id == message_id => Some(&self.well_known_folder_action_status),
            _ => None,
        }
    }
}

fn message_move_outcome_is_unknown(error: &MailWorkflowError) -> bool {
    error == &MailWorkflowError::Provider(MailOperationError::MoveOutcomeUnknown)
}
