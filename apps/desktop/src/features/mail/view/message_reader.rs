use einfach_mail_application::{MailWorkflowError, OpenedMessage, WellKnownMailFolder};
use einfach_mail_domain::{FolderReference, MailAction, MessageId};
use gpui_kit::component::{ColorTokens, StyledExt, button::Button};
#[cfg(test)]
use gpui_kit::test::TestSupportExt as _;
use gpui_kit::{AnyElement, Context, IntoElement, Role, div, prelude::*};

use super::super::body::render_message_body;
use super::{
    MailView, MessageDetailState, empty_message_detail_copy, mail_error_message, message_sender,
};

impl MailView {
    pub(super) fn render_message_detail(
        &self,
        colors: &ColorTokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let detail = match &self.message_detail_state {
            MessageDetailState::Empty => self.render_empty_message_detail(colors, cx),
            MessageDetailState::Loading(_) => Self::render_loading_message_detail(colors),
            MessageDetailState::Failed { error, .. } => {
                self.render_failed_message_detail(*error, cx)
            }
            MessageDetailState::Opened(opened) => {
                self.render_opened_message_detail(opened, colors, cx)
            }
            MessageDetailState::MoveCompleted {
                destination_folder,
                destination_label,
                open_action_label,
            } => Self::render_move_completed_detail(
                destination_folder,
                destination_label,
                open_action_label,
                cx,
            ),
            MessageDetailState::DeleteCompleted => Self::render_delete_completed_detail(),
        };
        Self::wrap_message_reader(detail)
    }

    fn render_empty_message_detail(
        &self,
        colors: &ColorTokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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

    fn render_loading_message_detail(colors: &ColorTokens) -> AnyElement {
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .items_center()
            .justify_center()
            .text_color(colors.muted_foreground)
            .child("Loading message…")
            .into_any_element()
    }

    fn render_failed_message_detail(
        &self,
        error: MailWorkflowError,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let detail_error = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .p_6()
            .id("mail-message-error")
            .role(Role::Alert)
            .aria_label(mail_error_message(error))
            .child(mail_error_message(error))
            .children(self.render_move_recovery_notice(cx));
        #[cfg(test)]
        let detail_error = detail_error.test_support();
        detail_error.into_any_element()
    }

    fn render_opened_message_detail(
        &self,
        opened: &OpenedMessage,
        colors: &ColorTokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let body = render_message_body(opened.body(), *colors);
        let header = self.render_opened_message_header(opened, colors, cx);
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn render_opened_message_header(
        &self,
        opened: &OpenedMessage,
        colors: &ColorTokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let message_id = opened.message().id();
        let controls = self.render_opened_message_controls(opened, message_id, cx);
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_6()
            .border_b_1()
            .border_color(colors.border)
            .child(controls)
            .child(
                div()
                    .text_sm()
                    .text_color(colors.muted_foreground)
                    .child(message_sender(opened.message())),
            )
            .children(
                self.read_state_update
                    .feedback_for(message_id)
                    .map(|feedback| self.render_read_state_feedback_element(feedback, cx)),
            )
            .children(self.render_follow_up_flag_feedback(message_id, cx))
            .children(self.render_message_move_feedback(message_id, cx))
            .children(self.render_well_known_folder_action_feedback(message_id, cx))
            .children(self.render_move_recovery_notice(cx))
            .children(self.render_read_state_reconciliation_notice())
            .into_any_element()
    }

    fn render_opened_message_controls(
        &self,
        opened: &OpenedMessage,
        message_id: &MessageId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let subject = opened
            .message()
            .metadata()
            .subject()
            .unwrap_or("(No subject)")
            .to_owned();
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(div().text_lg().font_bold().child(subject))
            .child(self.render_message_action_buttons(message_id, cx))
            .into_any_element()
    }

    fn render_message_action_buttons(
        &self,
        message_id: &MessageId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mail = self.profile.capabilities().mail();
        if mail.is_read_only() {
            return div().into_any_element();
        }
        let mut actions = div().flex().items_center().gap_2();
        if mail.supports(MailAction::SetReadState) {
            actions = actions.child(self.render_read_state_action(message_id, cx));
        }
        if mail.supports(MailAction::SetFollowUpFlagStatus) {
            actions = actions.child(self.render_follow_up_flag_action(message_id, cx));
        }
        if mail.supports(MailAction::Move) {
            actions = actions
                .child(self.render_well_known_folder_action(
                    WellKnownMailFolder::Archive,
                    message_id,
                    cx,
                ))
                .child(self.render_move_action(message_id, cx));
        }
        if mail.supports(MailAction::Reply) {
            actions = actions.child(self.render_reply_action(message_id, cx));
        }
        if mail.supports(MailAction::Delete) {
            actions = actions.child(self.render_well_known_folder_action(
                WellKnownMailFolder::DeletedItems,
                message_id,
                cx,
            ));
        }
        #[cfg(test)]
        let actions = actions.id("message-actions").test_support();
        actions.into_any_element()
    }

    fn render_move_completed_detail(
        destination_folder: &FolderReference,
        destination_label: &str,
        open_action_label: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
                    .label(open_action_label)
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

    fn render_delete_completed_detail() -> AnyElement {
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

    fn wrap_message_reader(detail: AnyElement) -> AnyElement {
        let reader = div()
            .id("mail-reader")
            .role(Role::Group)
            .aria_label("Message reader")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(detail);
        #[cfg(test)]
        let reader = reader.test_support();
        reader.into_any_element()
    }
}
