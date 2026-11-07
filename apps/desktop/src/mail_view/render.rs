use std::rc::Rc;

use crate::message_body::render_message_body;
use einfach_mail_application::{MailOperationError, MailWorkflowError, WellKnownMailFolder};
use einfach_mail_domain::{FolderReference, MailFolder, MessageId, ReadState};
use gpui_kit::base::v_virtual_list;
use gpui_kit::component::{
    ActiveTheme, ColorTokens, Disableable, Selectable, StyledExt,
    button::{Button, ButtonVariants},
    menu::{DropdownMenu, PopupMenu, PopupMenuItem},
    scroll::ScrollableElement,
};
#[cfg(test)]
use gpui_kit::test::TestSupportExt as _;
use gpui_kit::{
    AnyElement, Context, Div, IntoElement, Pixels, Render, Role, Stateful, WeakEntity, Window, div,
    prelude::*, px,
};

use super::{
    FollowUpFlagUpdateStatus, MESSAGE_LIST_CONTEXT, MESSAGE_ROW_HEIGHT, MailView, MailViewEvent,
    MessageDetailState, MessageListState, MessageMoveMenuAction, MessageMoveStatus,
    MoveRecoveryReason, ReadStateFeedback, SyncStatus, WellKnownFolderActionStatus,
    empty_message_detail_copy, follow_up_flag_cache_sync_message, follow_up_flag_confirmed_message,
    follow_up_flag_failure_message, follow_up_flag_progress_message, mail_error_message,
    message_sender,
};

const MESSAGE_LIST_WIDTH: Pixels = px(344.);

impl MailView {
    fn render_reply_action(&self, message_id: &MessageId, cx: &mut Context<Self>) -> AnyElement {
        Button::new("message-reply")
            .debug_selector(|| "message-reply".to_owned())
            .ghost()
            .label("Reply")
            .disabled(
                self.sync_running
                    || self.message_update_in_progress()
                    || self.pending_send.is_some()
                    || self.pending_send_error.is_some()
                    || self.selected_message_id.as_ref() != Some(message_id),
            )
            .on_click(cx.listener(|view, _, window, cx| {
                view.open_reply_composer(window, cx);
            }))
            .into_any_element()
    }

    fn render_header(&self, colors: ColorTokens, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.sync_status_label();
        let status_role = if matches!(self.sync_status, SyncStatus::Failed(_)) {
            Role::Alert
        } else {
            Role::Status
        };

        div()
            .w_full()
            .h(px(58.))
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .px_4()
            .border_b_1()
            .border_color(colors.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(div().text_lg().font_bold().child("Mail"))
                    .child(
                        div()
                            .id("mail-sync-status")
                            .text_sm()
                            .text_color(colors.muted_foreground)
                            .role(status_role)
                            .aria_label(status.clone())
                            .child(status),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("new-mail-message")
                            .debug_selector(|| "new-mail-message".to_owned())
                            .primary()
                            .label(if self.pending_send.is_some() {
                                "Resolve send"
                            } else {
                                "Compose"
                            })
                            .disabled(self.sync_running)
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.open_composer(window, cx);
                            })),
                    )
                    .child(
                        Button::new("sync-mail")
                            .ghost()
                            .label("Sync")
                            .loading(self.sync_running)
                            .disabled(self.sync_running || self.message_update_in_progress())
                            .on_click(cx.listener(|view, _, _, cx| view.refresh(cx))),
                    )
                    .child(
                        Button::new("account")
                            .debug_selector(|| "mail-account-navigation".to_owned())
                            .ghost()
                            .label("Account")
                            .disabled(self.message_update_in_progress())
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(MailViewEvent::BackToAccount);
                            })),
                    ),
            )
    }

    fn render_folder_sidebar(
        &self,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut folders = div().flex().flex_col().gap_1().overflow_y_scrollbar();
        for folder in &self.folders {
            let reference = folder.reference().clone();
            folders = folders.child(
                Button::new(format!("folder:{}", reference.folder_id().as_str()))
                    .ghost()
                    .selected(self.selected_folder.as_ref() == Some(&reference))
                    .label(folder.display_name().to_owned())
                    .disabled(self.sync_running || self.message_update_in_progress())
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.select_folder(reference.clone(), cx);
                    })),
            );
        }
        div()
            .w(px(216.))
            .min_h_0()
            .flex()
            .flex_col()
            .gap_3()
            .p_3()
            .bg(colors.surface)
            .child(div().text_sm().font_bold().child("Folders"))
            .child(folders.flex_1().min_h_0())
    }

    fn render_message_list(&self, colors: ColorTokens, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(folder) = self.selected_folder.as_ref() else {
            return div()
                .w(MESSAGE_LIST_WIDTH)
                .min_h_0()
                .p_4()
                .text_color(colors.muted_foreground)
                .child("No visible mail folders were found.")
                .into_any_element();
        };
        match &self.message_list_state {
            MessageListState::Ready => {}
            MessageListState::Loading => {
                return div()
                    .w(MESSAGE_LIST_WIDTH)
                    .min_h_0()
                    .p_4()
                    .id("mail-list-loading")
                    .role(Role::Status)
                    .child("Loading messages…")
                    .into_any_element();
            }
            MessageListState::Failed(error) => {
                let list_error = div()
                    .w(MESSAGE_LIST_WIDTH)
                    .min_h_0()
                    .p_4()
                    .id("mail-list-error")
                    .role(Role::Alert)
                    .aria_label(mail_error_message(*error))
                    .child(mail_error_message(*error));
                #[cfg(test)]
                let list_error = list_error.test_support();
                return list_error.into_any_element();
            }
        }
        if self.messages.is_empty() {
            return div()
                .w(MESSAGE_LIST_WIDTH)
                .min_h_0()
                .p_4()
                .text_color(colors.muted_foreground)
                .child("No messages in this folder.")
                .into_any_element();
        }

        let folder_reference = folder.clone();
        let sizes = Rc::clone(&self.message_row_sizes);
        div()
            .w(MESSAGE_LIST_WIDTH)
            .min_h_0()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(colors.border)
            .child(div().px_4().py_3().text_sm().font_bold().child("Messages"))
            .child({
                let message_list = div().id("cached-message-list").flex_1().min_h_0();
                #[cfg(test)]
                let message_list = message_list.test_support();
                message_list
                    .track_focus(&self.message_list_focus)
                    .key_context(MESSAGE_LIST_CONTEXT)
                    .role(Role::ListBox)
                    .aria_label("Messages")
                    .on_action(cx.listener(Self::on_select_previous_message))
                    .on_action(cx.listener(Self::on_select_next_message))
                    .on_action(cx.listener(Self::on_select_first_message))
                    .on_action(cx.listener(Self::on_select_last_message))
                    .child(
                        v_virtual_list(
                            cx.entity(),
                            "cached-message-options",
                            sizes,
                            move |view, range, _, cx| {
                                let list_size = view.messages.len();
                                range
                                    .filter_map(|index| {
                                        let message = view.messages.get(index)?;
                                        let message_id = message.id().clone();
                                        let selected =
                                            view.selected_message_id.as_ref() == Some(&message_id);
                                        let sender = message_sender(message);
                                        let subject = message
                                            .metadata()
                                            .subject()
                                            .unwrap_or("(No subject)")
                                            .to_owned();
                                        let accessible_label = format!("{sender}. {subject}");
                                        let expected_folder = folder_reference.clone();
                                        let option = div()
                                            .id(format!("message-option-{}", message_id.as_str()))
                                            .debug_selector(|| {
                                                format!("message-row-{}", message_id.as_str())
                                            })
                                            .w_full()
                                            .h(MESSAGE_ROW_HEIGHT)
                                            .px_3()
                                            .py_3()
                                            .rounded_md()
                                            .text_color(colors.foreground)
                                            .role(Role::ListBoxOption)
                                            .aria_label(if view.message_update_in_progress() {
                                                format!("{accessible_label}, mail action pending")
                                            } else {
                                                accessible_label
                                            })
                                            .aria_position_in_set(index + 1)
                                            .aria_size_of_set(list_size)
                                            .aria_selected(selected)
                                            .when(selected, |option| {
                                                option
                                                    .aria_active_descendant()
                                                    .bg(colors.accent)
                                                    .text_color(colors.accent_foreground)
                                            })
                                            .hover(|option| {
                                                option.bg(if selected {
                                                    colors.accent
                                                } else {
                                                    colors.muted
                                                })
                                            })
                                            .child(
                                                div()
                                                    .w_full()
                                                    .flex()
                                                    .flex_col()
                                                    .gap_1()
                                                    .child(
                                                        div()
                                                            .w_full()
                                                            .text_sm()
                                                            .font_bold()
                                                            .child(sender),
                                                    )
                                                    .child(div().w_full().text_sm().child(subject)),
                                            )
                                            .when(!view.message_update_in_progress(), |option| {
                                                option.on_click(cx.listener(
                                                    move |view, _, window, cx| {
                                                        view.select_message_from_option(
                                                            message_id.clone(),
                                                            &expected_folder,
                                                            window,
                                                            cx,
                                                        );
                                                    },
                                                ))
                                            });
                                        #[cfg(test)]
                                        let option = option.test_support();
                                        Some(option)
                                    })
                                    .collect()
                            },
                        )
                        .track_scroll(&self.message_list_scroll),
                    )
                    .focus_visible(|style| style.border_1().border_color(colors.ring))
            })
            .into_any_element()
    }

    fn render_message_detail(
        &self,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        match &self.message_detail_state {
            MessageDetailState::Empty => {
                let empty_state = empty_message_detail_copy(
                    &self.message_list_state,
                    self.selected_folder.as_ref(),
                    &self.messages,
                );
                let panel = div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .text_color(colors.muted_foreground);
                if let Some(notice) = self.render_move_recovery_notice(cx) {
                    panel.child(notice).into_any_element()
                } else {
                    panel.child(empty_state).into_any_element()
                }
            }
            MessageDetailState::Loading => div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .items_center()
                .justify_center()
                .text_color(colors.muted_foreground)
                .child("Loading message…")
                .into_any_element(),
            MessageDetailState::Failed(error) => {
                let detail_error = div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .p_6()
                    .id("mail-message-error")
                    .role(Role::Alert)
                    .aria_label(mail_error_message(*error))
                    .child(mail_error_message(*error))
                    .children(self.render_move_recovery_notice(cx));
                #[cfg(test)]
                let detail_error = detail_error.test_support();
                detail_error.into_any_element()
            }
            MessageDetailState::Opened(opened) => {
                let metadata = opened.message().metadata();
                let message_id = opened.message().id();
                let body = render_message_body(opened.body(), colors);
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .p_6()
                            .border_b_1()
                            .border_color(colors.border)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_3()
                                    .child(div().text_lg().font_bold().child(
                                        metadata.subject().unwrap_or("(No subject)").to_owned(),
                                    ))
                                    .child({
                                        let actions = div()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(self.render_read_state_action(message_id, cx))
                                            .child(
                                                self.render_follow_up_flag_action(message_id, cx),
                                            )
                                            .child(self.render_well_known_folder_action(
                                                WellKnownMailFolder::Archive,
                                                message_id,
                                                cx,
                                            ))
                                            .child(self.render_move_action(message_id, cx))
                                            .child(self.render_reply_action(message_id, cx))
                                            .child(self.render_well_known_folder_action(
                                                WellKnownMailFolder::DeletedItems,
                                                message_id,
                                                cx,
                                            ));
                                        #[cfg(test)]
                                        let actions = actions.id("message-actions").test_support();
                                        actions
                                    }),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(colors.muted_foreground)
                                    .child(message_sender(opened.message())),
                            )
                            .children(self.read_state_update.feedback_for(message_id).map(
                                |feedback| self.render_read_state_feedback_element(feedback, cx),
                            ))
                            .children(self.render_follow_up_flag_feedback(message_id, cx))
                            .children(self.render_message_move_feedback(message_id, cx))
                            .children(self.render_well_known_folder_action_feedback(message_id, cx))
                            .children(self.render_move_recovery_notice(cx))
                            .children(self.render_read_state_reconciliation_notice()),
                    )
                    .child(body)
                    .into_any_element()
            }
            MessageDetailState::MoveCompleted {
                destination_folder,
                destination_label,
                open_action_label,
            } => {
                let completed = div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .id("message-move-completed")
                    .debug_selector(|| "message-move-completed".to_owned())
                    .role(Role::Status)
                    .child(format!("Moved to {destination_label}."))
                    .child(
                        Button::new("open-moved-message-folder")
                            .debug_selector(|| "open-moved-message-folder".to_owned())
                            .label(*open_action_label)
                            .on_click(cx.listener({
                                let destination_folder = destination_folder.clone();
                                move |view, _, _, cx| {
                                    view.select_folder(destination_folder.clone(), cx);
                                }
                            })),
                    );
                #[cfg(test)]
                let completed = completed.test_support();
                completed.into_any_element()
            }
            MessageDetailState::DeleteCompleted => {
                let completed = div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .id("message-delete-completed")
                    .debug_selector(|| "message-delete-completed".to_owned())
                    .role(Role::Status)
                    .child("Moved to Deleted Items.");
                #[cfg(test)]
                let completed = completed.test_support();
                completed.into_any_element()
            }
        }
    }

    fn render_move_action(&self, message_id: &MessageId, cx: &mut Context<Self>) -> AnyElement {
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
            .disabled(!self.message_move_allowed() || destinations.is_empty())
            .dropdown_menu(move |menu, _, _| {
                build_message_move_menu(
                    menu,
                    destinations.clone(),
                    entity.clone(),
                    selected_message_id.clone(),
                )
            })
            .into_any_element()
    }

    fn render_well_known_folder_action(
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
        let button = Button::new(button_id)
            .debug_selector(|| button_id.to_owned())
            .label(if pending { pending_label } else { label })
            .loading(pending)
            .disabled(
                !self.message_move_allowed()
                    || self.selected_message_id.as_ref() != Some(message_id),
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

    fn render_message_move_feedback(
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
            _ => None,
        }
    }

    fn render_well_known_folder_action_feedback(
        &self,
        message_id: &MessageId,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        match self.well_known_folder_action_status_for(message_id)? {
            WellKnownFolderActionStatus::Pending { destination, .. } => {
                Some(self.render_well_known_folder_action_pending(*destination))
            }
            WellKnownFolderActionStatus::Failed {
                destination, error, ..
            } => Some(self.render_well_known_folder_action_error(*destination, *error, cx)),
            WellKnownFolderActionStatus::Idle => None,
        }
    }

    fn render_well_known_folder_action_pending(
        &self,
        destination: WellKnownMailFolder,
    ) -> AnyElement {
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
            ) {
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

    fn render_move_recovery_notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let reason = self.move_recovery_reason?;
        if self.sync_running && matches!(reason, MoveRecoveryReason::InitialSyncRequired) {
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
                    .disabled(self.sync_running)
                    .on_click(cx.listener(|view, _, _, cx| view.refresh(cx))),
            );
        #[cfg(test)]
        let recovery = recovery.test_support();
        recovery.into_any_element()
    }

    fn render_read_state_action(
        &self,
        message_id: &MessageId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let read_state = self.cached_read_state(message_id);
        let pending = self.read_state_update.is_pending_for(message_id);
        let label = match (pending, read_state) {
            (true, _) => "Updating…",
            (false, Some(ReadState::Read)) => "Mark unread",
            (false, Some(ReadState::Unread)) => "Mark read",
            (false, None) => "Mark read",
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

    fn render_follow_up_flag_action(
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
        self.selected_message_id.as_ref() != Some(message_id)
            || self
                .selected_folder
                .as_ref()
                .is_none_or(|folder| !self.can_update_message_in(folder))
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
        read_state.is_none() || !self.can_update_message_in(folder)
    }

    fn render_read_state_feedback_element(
        &self,
        feedback: ReadStateFeedback,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (message, role, show_permission_review) = feedback.into_parts();
        let feedback_view = self.read_state_feedback_view(message, role, cx);
        if show_permission_review {
            feedback_view
                .flex()
                .flex_col()
                .gap_2()
                .child(self.render_permission_review_button(cx))
                .into_any_element()
        } else {
            feedback_view.into_any_element()
        }
    }

    fn read_state_feedback_view(
        &self,
        message: String,
        role: Role,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let colors = cx.theme().semantic_tokens().colors;
        div()
            .id("message-read-state-feedback")
            .debug_selector(|| "message-read-state-feedback".to_owned())
            .role(role)
            .text_sm()
            .text_color(if role == Role::Alert {
                colors.destructive
            } else {
                colors.muted_foreground
            })
            .child(message)
    }

    fn render_permission_review_button(&self, cx: &mut Context<Self>) -> AnyElement {
        Button::new("review-mail-permissions-from-message")
            .ghost()
            .debug_selector(|| "review-mail-permissions-from-message".to_owned())
            .label("Review mail permissions…")
            .on_click(cx.listener(|_, _, _, cx| {
                cx.emit(MailViewEvent::ReviewMailWritePermission);
            }))
            .into_any_element()
    }

    fn render_follow_up_flag_feedback(
        &self,
        message_id: &MessageId,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (message, role) = match &self.follow_up_flag_update {
            FollowUpFlagUpdateStatus::Idle => return None,
            FollowUpFlagUpdateStatus::Pending {
                message_id: pending_id,
                requested,
            } if pending_id == message_id => {
                (follow_up_flag_progress_message(*requested), Role::Status)
            }
            FollowUpFlagUpdateStatus::Confirmed {
                message_id: confirmed_id,
                status,
            } if confirmed_id == message_id => (
                follow_up_flag_confirmed_message(*status).to_owned(),
                Role::Status,
            ),
            FollowUpFlagUpdateStatus::ConfirmedNeedsSync {
                message_id: confirmed_id,
                status,
            } if confirmed_id == message_id => (
                follow_up_flag_cache_sync_message(*status).to_owned(),
                Role::Status,
            ),
            FollowUpFlagUpdateStatus::Failed {
                message_id: failed_id,
                requested,
                error,
            } if failed_id == message_id => (
                follow_up_flag_failure_message(*requested, *error),
                Role::Alert,
            ),
            _ => return None,
        };
        let colors = cx.theme().semantic_tokens().colors;
        let feedback = div()
            .id("message-follow-up-flag-feedback")
            .debug_selector(|| "message-follow-up-flag-feedback".to_owned())
            .role(role)
            .text_sm()
            .text_color(if role == Role::Alert {
                colors.destructive
            } else {
                colors.muted_foreground
            })
            .child(message);
        if matches!(
            &self.follow_up_flag_update,
            FollowUpFlagUpdateStatus::Failed {
                message_id: failed_id,
                error: MailWorkflowError::Provider(MailOperationError::AccessDenied),
                ..
            } if failed_id == message_id
        ) {
            Some(
                feedback
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        Button::new("review-mail-permissions-from-flag")
                            .ghost()
                            .debug_selector(|| "review-mail-permissions-from-flag".to_owned())
                            .label("Review mail permissions…")
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(MailViewEvent::ReviewMailWritePermission);
                            })),
                    )
                    .into_any_element(),
            )
        } else {
            Some(feedback.into_any_element())
        }
    }

    fn render_read_state_reconciliation_notice(&self) -> Option<AnyElement> {
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
    entity: WeakEntity<MailView>,
    selected_message_id: MessageId,
) -> PopupMenu {
    destinations
        .into_iter()
        .fold(menu.scrollable(true), |menu, destination| {
            menu.item(message_move_menu_item(
                destination,
                entity.clone(),
                selected_message_id.clone(),
            ))
        })
}

fn message_move_menu_item(
    (destination_folder, destination_label): (FolderReference, String),
    entity: WeakEntity<MailView>,
    selected_message_id: MessageId,
) -> PopupMenuItem {
    let action = MessageMoveMenuAction {
        selected_message_id,
        destination_folder,
        destination_label: destination_label.clone(),
    };
    PopupMenuItem::new(destination_label).on_click(move |_, _, cx| {
        if let Some(view) = entity.upgrade() {
            view.update(cx, |view, cx| {
                view.apply_message_move_menu_action(action.clone(), cx)
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

impl Render for MailView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().semantic_tokens().colors;
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.background)
            .text_color(colors.foreground)
            .child(self.render_header(colors, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.render_folder_sidebar(colors, cx))
                    .child(self.render_message_list(colors, cx))
                    .child(self.render_message_detail(colors, cx)),
            )
    }
}

fn well_known_folder_progress_message(folder: WellKnownMailFolder) -> &'static str {
    match folder {
        WellKnownMailFolder::Archive => "Moving message to Archive…",
        WellKnownMailFolder::DeletedItems => "Moving message to Deleted Items…",
    }
}
