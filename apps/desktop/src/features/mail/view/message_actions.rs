use einfach_mail_application::{MailWorkflowError, WellKnownMailFolder};
use einfach_mail_domain::{FolderReference, MailAction, MailFolder, MessageId, ReadState};
use gpui_kit::component::{
    ActiveTheme, Disableable,
    button::{Button, ButtonVariants},
    menu::{DropdownMenu, PopupMenu, PopupMenuItem},
};
#[cfg(test)]
use gpui_kit::test::TestSupportExt as _;
use gpui_kit::{
    AnyElement, Context, Div, IntoElement, Role, Stateful, WeakEntity, div, prelude::*,
};

use super::message_write::{MessageWriteFeedback, MessageWriteFeedbackAction};
use super::{
    MailView, MailViewEvent, MessageMoveMenuAction, MessageMoveStatus, MoveRecoveryReason,
    SyncStatus, WellKnownFolderActionStatus, mail_error_message, well_known_folder_mail_action,
};

impl MailView {
    pub(super) fn render_reply_action(
        &self,
        message_id: &MessageId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        Button::new("message-reply")
            .debug_selector(|| "message-reply".to_owned())
            .ghost()
            .label("Reply")
            .disabled(
                self.sync_status.is_sync_active()
                    || self.message_update_in_progress()
                    || !matches!(&self.pending_send, Ok(None))
                    || self.selected_message_id() != Some(message_id),
            )
            .on_click(cx.listener(|view, _, window, cx| {
                view.open_reply_composer(window, cx);
            }))
            .into_any_element()
    }

    pub(super) fn render_move_action(
        &self,
        message_id: &MessageId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(source_folder) = self.selected_folder.as_ref() else {
            return Button::new("message-move")
                .label("Move to…")
                .disabled(true)
                .into_any_element();
        };
        let destinations = self.move_destinations(source_folder);
        let selected_message_id = message_id.clone();
        let entity = cx.entity().downgrade();
        let pending = self.message_move_pending_for(message_id);
        Button::new("message-move")
            .ghost()
            .debug_selector(|| "message-move".to_owned())
            .label(if pending { "Moving…" } else { "Move to…" })
            .loading(pending)
            .disabled(!self.message_move_allowed(MailAction::Move) || destinations.is_empty())
            .dropdown_menu(move |menu, _, _| {
                build_message_move_menu(menu, destinations.clone(), &entity, &selected_message_id)
            })
            .into_any_element()
    }

    pub(super) fn render_well_known_folder_action(
        &self,
        destination: WellKnownMailFolder,
        message_id: &MessageId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pending = self.well_known_folder_action_in_progress_for(message_id, destination);
        let (button_id, label, pending_label) = match destination {
            WellKnownMailFolder::Archive => ("message-archive", "Archive", "Archiving…"),
            WellKnownMailFolder::DeletedItems => ("message-delete", "Delete", "Deleting…"),
        };
        let action = well_known_folder_mail_action(destination);
        let button = Button::new(button_id)
            .debug_selector(|| button_id.to_owned())
            .label(if pending { pending_label } else { label })
            .loading(pending)
            .disabled(
                !self.message_move_allowed(action)
                    || self.selected_message_id() != Some(message_id),
            )
            .on_click(cx.listener(move |view, _, _, cx| {
                view.start_well_known_folder_action(destination, cx);
            }));
        match destination {
            WellKnownMailFolder::Archive => button.ghost().into_any_element(),
            WellKnownMailFolder::DeletedItems => button.danger().into_any_element(),
        }
    }

    fn move_destinations(&self, source_folder: &FolderReference) -> Vec<(FolderReference, String)> {
        self.folders
            .iter()
            .filter(|folder| {
                folder.reference() != source_folder
                    && folder.reference().account_id() == source_folder.account_id()
            })
            .map(|folder| (folder.reference().clone(), self.folder_menu_label(folder)))
            .collect()
    }

    fn folder_menu_label(&self, folder: &MailFolder) -> String {
        let mut labels = vec![folder.display_name()];
        let mut parent = folder.parent();
        let mut remaining = self.folders.len();
        while let Some(parent_reference) = parent {
            if remaining == 0 {
                break;
            }
            remaining -= 1;
            let Some(parent_folder) = self
                .folders
                .iter()
                .find(|candidate| candidate.reference() == parent_reference)
            else {
                break;
            };
            labels.push(parent_folder.display_name());
            parent = parent_folder.parent();
        }
        labels.reverse();
        labels.join(" / ")
    }

    pub(super) fn render_message_move_feedback(
        &self,
        message_id: &MessageId,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        match self.message_move_status_for(message_id)? {
            MessageMoveStatus::Pending {
                destination_label, ..
            } => Some(render_message_pending_feedback(
                "message-move-pending",
                format!("Moving to {destination_label}…"),
            )),
            MessageMoveStatus::Failed { error, .. } => Some(self.render_message_action_error(
                "message-move-error",
                "review-permissions-from-move",
                *error,
                cx,
            )),
            MessageMoveStatus::Idle => None,
        }
    }

    pub(super) fn render_well_known_folder_action_feedback(
        &self,
        message_id: &MessageId,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        match self.well_known_folder_action_status_for(message_id)? {
            WellKnownFolderActionStatus::Pending { destination, .. } => {
                Some(Self::render_well_known_folder_action_pending(*destination))
            }
            WellKnownFolderActionStatus::Failed {
                destination, error, ..
            } => Some(self.render_well_known_folder_action_error(*destination, *error, cx)),
            WellKnownFolderActionStatus::Idle => None,
        }
    }

    fn render_well_known_folder_action_pending(destination: WellKnownMailFolder) -> AnyElement {
        let pending_id = match destination {
            WellKnownMailFolder::Archive => "message-archive-pending",
            WellKnownMailFolder::DeletedItems => "message-delete-pending",
        };
        render_message_pending_feedback(
            pending_id,
            well_known_folder_progress_message(destination).to_owned(),
        )
    }

    fn render_well_known_folder_action_error(
        &self,
        destination: WellKnownMailFolder,
        error: MailWorkflowError,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (error_id, permission_button_id) = match destination {
            WellKnownMailFolder::Archive => {
                ("message-archive-error", "review-permissions-from-archive")
            }
            WellKnownMailFolder::DeletedItems => {
                ("message-delete-error", "review-permissions-from-delete")
            }
        };
        self.render_message_action_error(error_id, permission_button_id, error, cx)
    }

    fn render_message_action_error(
        &self,
        error_id: &'static str,
        permission_button_id: &'static str,
        error: MailWorkflowError,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let feedback = div()
            .id(error_id)
            .debug_selector(|| error_id.to_owned())
            .role(Role::Alert)
            .flex()
            .flex_col()
            .gap_2()
            .child(mail_error_message(error));
        let feedback = if error
            == MailWorkflowError::Provider(
                einfach_mail_application::MailOperationError::AccessDenied,
            )
            && self
                .profile
                .capabilities()
                .can_review_mail_write_permission()
        {
            feedback.child(
                Button::new(permission_button_id)
                    .ghost()
                    .debug_selector(|| permission_button_id.to_owned())
                    .label("Review permissions…")
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.emit(MailViewEvent::ReviewMailWritePermission);
                    })),
            )
        } else {
            feedback
        };
        #[cfg(test)]
        let feedback = feedback.test_support();
        feedback.into_any_element()
    }

    pub(super) fn render_move_recovery_notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let reason = self.move_recovery_reason?;
        if self.sync_status.is_sync_active()
            && matches!(reason, MoveRecoveryReason::InitialSyncRequired)
        {
            return None;
        }
        Some(self.render_move_recovery_notice_element(reason, cx))
    }

    fn render_move_recovery_notice_element(
        &self,
        reason: MoveRecoveryReason,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let recovery = div()
            .id("message-move-recovery")
            .debug_selector(|| "message-move-recovery".to_owned())
            .role(Role::Alert)
            .flex()
            .flex_col()
            .gap_2()
            .child(move_recovery_message(reason))
            .child(
                Button::new("sync-after-message-move")
                    .debug_selector(|| "sync-after-message-move".to_owned())
                    .ghost()
                    .label("Sync mail")
                    .disabled(self.sync_status.is_sync_active())
                    .on_click(cx.listener(|view, _, _, cx| view.refresh(cx))),
            );
        #[cfg(test)]
        let recovery = recovery.test_support();
        recovery.into_any_element()
    }

    pub(super) fn render_read_state_action(
        &self,
        message_id: &MessageId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let read_state = self.cached_read_state(message_id);
        let pending = self.read_state_update.is_pending_for(message_id);
        let label = match (pending, read_state) {
            (true, _) => "Updating…",
            (false, Some(ReadState::Read)) => "Mark unread",
            (false, Some(ReadState::Unread) | None) => "Mark read",
        };
        Button::new("message-read-state")
            .ghost()
            .debug_selector(|| "message-read-state".to_owned())
            .label(label)
            .loading(pending)
            .disabled(self.read_state_action_disabled(read_state))
            .on_click(cx.listener(|view, _, _, cx| view.update_selected_read_state(cx)))
            .into_any_element()
    }

    pub(super) fn render_follow_up_flag_action(
        &self,
        message_id: &MessageId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let flag_status = self
            .messages
            .iter()
            .find(|message| message.id() == message_id)
            .and_then(|message| message.metadata().follow_up_flag_status());
        Button::new("message-follow-up-flag")
            .ghost()
            .debug_selector(|| "message-follow-up-flag".to_owned())
            .label(
                self.follow_up_flag_update
                    .action_label(message_id, flag_status),
            )
            .loading(self.follow_up_flag_update.is_pending_for(message_id))
            .disabled(self.follow_up_flag_action_disabled(message_id))
            .on_click(cx.listener(|view, _, _, cx| view.update_selected_follow_up_flag(cx)))
            .into_any_element()
    }

    fn follow_up_flag_action_disabled(&self, message_id: &MessageId) -> bool {
        self.selected_message_id() != Some(message_id)
            || self.selected_folder.as_ref().is_none_or(|folder| {
                !self.can_update_message_in(folder, MailAction::SetFollowUpFlagStatus)
            })
    }

    fn cached_read_state(&self, message_id: &MessageId) -> Option<ReadState> {
        self.messages
            .iter()
            .find(|message| message.id() == message_id)
            .map(|message| message.metadata().read_state())
    }

    fn read_state_action_disabled(&self, read_state: Option<ReadState>) -> bool {
        let Some(folder) = self.selected_folder.as_ref() else {
            return true;
        };
        read_state.is_none() || !self.can_update_message_in(folder, MailAction::SetReadState)
    }

    pub(super) fn render_read_state_feedback_element(
        &self,
        feedback: MessageWriteFeedback,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.render_message_write_feedback(
            feedback,
            "message-read-state-feedback",
            "review-mail-permissions-from-message",
            cx,
        )
    }

    fn render_message_write_feedback(
        &self,
        feedback: MessageWriteFeedback,
        feedback_id: &'static str,
        permission_review_id: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let MessageWriteFeedback {
            message,
            role,
            action,
        } = feedback;
        let feedback_view = Self::message_write_feedback_view(message, role, feedback_id, cx);
        let permission_review_id =
            action.map(|MessageWriteFeedbackAction::ReviewPermission| permission_review_id);
        self.with_permission_review(feedback_view, permission_review_id, cx)
    }

    fn message_write_feedback_view(
        message: String,
        role: Role,
        feedback_id: &'static str,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let colors = cx.theme().semantic_tokens().colors;
        div()
            .id(feedback_id)
            .debug_selector(move || feedback_id.to_owned())
            .role(role)
            .text_sm()
            .text_color(if role == Role::Alert {
                colors.destructive
            } else {
                colors.muted_foreground
            })
            .child(message)
    }

    fn with_permission_review(
        &self,
        feedback_view: Stateful<Div>,
        permission_review_id: Option<&'static str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match permission_review_id {
            Some(id)
                if self
                    .profile
                    .capabilities()
                    .can_review_mail_write_permission() =>
            {
                feedback_view
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(Self::render_permission_review_button(id, cx))
                    .into_any_element()
            }
            _ => feedback_view.into_any_element(),
        }
    }

    fn render_permission_review_button(id: &'static str, cx: &mut Context<Self>) -> AnyElement {
        Button::new(id)
            .ghost()
            .debug_selector(move || id.to_owned())
            .label("Review mail permissions…")
            .on_click(cx.listener(|_, _, _, cx| {
                cx.emit(MailViewEvent::ReviewMailWritePermission);
            }))
            .into_any_element()
    }

    pub(super) fn render_follow_up_flag_feedback(
        &self,
        message_id: &MessageId,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let feedback = self.follow_up_flag_update.feedback_for(message_id)?;
        Some(self.render_message_write_feedback(
            feedback,
            "message-follow-up-flag-feedback",
            "review-mail-permissions-from-flag",
            cx,
        ))
    }

    pub(super) fn render_read_state_reconciliation_notice(&self) -> Option<AnyElement> {
        let message = self.read_state_reconciliation_message()?;
        Some(
            div()
                .id("read-state-reconciliation")
                .debug_selector(|| "read-state-reconciliation".to_owned())
                .role(Role::Alert)
                .text_sm()
                .child(message)
                .into_any_element(),
        )
    }

    fn read_state_reconciliation_message(&self) -> Option<&'static str> {
        self.selected_folder.as_ref()?;
        if self.selected_folder_write_sync_required {
            Some(
                "A message change in this folder needs syncing. Sync mail before changing read state, follow-up flags, moving, archiving, or deleting messages again.",
            )
        } else if matches!(self.sync_status, SyncStatus::Failed(_)) {
            Some(
                "Mail sync failed. Retry sync before changing read state, follow-up flags, moving, archiving, or deleting messages.",
            )
        } else {
            None
        }
    }
}

fn build_message_move_menu(
    menu: PopupMenu,
    destinations: Vec<(FolderReference, String)>,
    entity: &WeakEntity<MailView>,
    selected_message_id: &MessageId,
) -> PopupMenu {
    destinations
        .into_iter()
        .fold(menu.scrollable(true), |menu, destination| {
            menu.item(message_move_menu_item(
                destination,
                entity,
                selected_message_id,
            ))
        })
}

fn message_move_menu_item(
    (destination_folder, destination_label): (FolderReference, String),
    entity: &WeakEntity<MailView>,
    selected_message_id: &MessageId,
) -> PopupMenuItem {
    let action = MessageMoveMenuAction {
        selected_message_id: (*selected_message_id).clone(),
        destination_folder,
        destination_label: destination_label.clone(),
    };
    let entity = (*entity).clone();
    PopupMenuItem::new(destination_label).on_click(move |_, _, cx| {
        if let Some(view) = entity.upgrade() {
            view.update(cx, |view, cx| {
                view.apply_message_move_menu_action(action.clone(), cx);
            });
        }
    })
}

fn move_recovery_message(reason: MoveRecoveryReason) -> &'static str {
    match reason {
        MoveRecoveryReason::InitialSyncRequired => super::ACCOUNT_SYNC_REQUIRED_MESSAGE,
        MoveRecoveryReason::OutcomeUnknown => {
            "The message may have moved. Sync mail before trying again."
        }
        MoveRecoveryReason::CacheNeedsSync => {
            "The message move was confirmed, but the local cache needs synchronization."
        }
    }
}

fn render_message_pending_feedback(selector: &'static str, message: String) -> AnyElement {
    let feedback = div()
        .id(selector)
        .debug_selector(|| selector.to_owned())
        .role(Role::Status)
        .child(message);
    #[cfg(test)]
    let feedback = feedback.test_support();
    feedback.into_any_element()
}

fn well_known_folder_progress_message(folder: WellKnownMailFolder) -> &'static str {
    match folder {
        WellKnownMailFolder::Archive => "Moving message to Archive…",
        WellKnownMailFolder::DeletedItems => "Moving message to Deleted Items…",
    }
}
