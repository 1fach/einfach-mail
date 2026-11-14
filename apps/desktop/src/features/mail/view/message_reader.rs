use einfach_mail_application::{MailWorkflowError, OpenedMessage};
use einfach_mail_domain::{FolderReference, MailAction, MailFolderKind};
use gpui_kit::component::{ColorTokens, StyledExt, button::Button};
#[cfg(test)]
use gpui_kit::test::TestSupportExt as _;
use gpui_kit::{
    AnyElement, Context, IntoElement, Pixels, Rems, Role, Window, div, prelude::*, rems,
};

use crate::components::{CommandBar, InfoBar, NAVIGATION_PANE_WIDTH};

use super::super::body::render_message_body;
use super::{
    MailView, MessageDetailState, empty_message_detail_copy, mail_error_message, message_sender,
};

/// The reader is narrower than this when the command bar folds its secondary commands into the
/// More menu.
const COMMAND_BAR_OVERFLOW_WIDTH: Rems = rems(40.);

/// Where the reader sits, which decides how its command bar is laid out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ReaderLayout {
    compact: bool,
    overflow: bool,
}

impl ReaderLayout {
    /// The layout for a window `window_width` wide.
    ///
    /// Beside the message list the reader gets what the navigation pane and the list leave. In
    /// compact mode it gets what the navigation pane leaves.
    pub(super) fn new(
        window_width: Pixels,
        rem_size: Pixels,
        message_list_width: Pixels,
        compact: bool,
    ) -> Self {
        let beside_navigation = window_width - NAVIGATION_PANE_WIDTH.to_pixels(rem_size);
        let reader_width = if compact {
            beside_navigation
        } else {
            beside_navigation - message_list_width
        };
        Self {
            compact,
            overflow: reader_width < COMMAND_BAR_OVERFLOW_WIDTH.to_pixels(rem_size),
        }
    }
}

impl MailView {
    /// The layout of the reader in `window`.
    pub(super) fn reader_layout(
        &self,
        window: &Window,
        rem_size: Pixels,
        compact: bool,
    ) -> ReaderLayout {
        ReaderLayout::new(
            window.bounds().size.width,
            rem_size,
            self.pane_layout.message_list_width.0.to_pixels(rem_size),
            compact,
        )
    }

    pub(super) fn render_message_detail(
        &self,
        colors: &ColorTokens,
        layout: ReaderLayout,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let detail = match &self.message_detail_state {
            MessageDetailState::Empty => self.render_empty_message_detail(colors, cx),
            MessageDetailState::Loading(_) => Self::render_loading_message_detail(colors),
            MessageDetailState::Failed { error, .. } => {
                self.render_failed_message_detail(*error, cx)
            }
            MessageDetailState::Opened(opened) => {
                self.render_opened_message_detail(opened, colors, layout, cx)
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
            panel
                .child(div().w_full().max_w(rems(32.)).px_6().child(notice))
                .into_any_element()
        } else {
            let label = div().id("mail-reader-empty-state").child(empty_state);
            #[cfg(test)]
            let label = label.test_support();
            panel.child(label).into_any_element()
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
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .p_6()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                InfoBar::alert("mail-message-error", mail_error_message(error))
                    .accessible_label(mail_error_message(error)),
            )
            .children(self.render_move_recovery_notice(cx))
            .into_any_element()
    }

    fn render_opened_message_detail(
        &self,
        opened: &OpenedMessage,
        colors: &ColorTokens,
        layout: ReaderLayout,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let body = render_message_body(opened.body(), *colors);
        let header = self.render_opened_message_header(opened, colors, cx);
        let command_bar = if layout.compact {
            None
        } else {
            self.message_command_bar(layout, cx).map(|bar| {
                div()
                    .w_full()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(colors.border)
                    .child(bar)
            })
        };
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .children(command_bar)
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
        let subject = opened
            .message()
            .metadata()
            .subject()
            .unwrap_or("(No subject)")
            .to_owned();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .px_6()
            .py_4()
            .border_b_1()
            .border_color(colors.border)
            .child(div().text_xl().font_semibold().child(subject))
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

    /// The commands the capabilities of the account allow on the open message, or `None` when the
    /// reader has no message open or the account cannot change mail.
    pub(super) fn message_command_bar(
        &self,
        layout: ReaderLayout,
        cx: &mut Context<Self>,
    ) -> Option<CommandBar> {
        let MessageDetailState::Opened(opened) = &self.message_detail_state else {
            return None;
        };
        let mail = self.profile.capabilities().mail();
        if mail.is_read_only() {
            return None;
        }
        let message_id = opened.message().id();
        let mut commands = Vec::new();
        if mail.supports(MailAction::Reply) {
            commands.push(self.reply_command(message_id, cx));
        }
        if mail.supports(MailAction::Move) {
            commands.push(self.well_known_folder_command(MailFolderKind::Archive, message_id, cx));
            commands.push(self.move_command(message_id, cx));
        }
        if mail.supports(MailAction::Delete) {
            commands.push(self.well_known_folder_command(
                MailFolderKind::DeletedItems,
                message_id,
                cx,
            ));
        }
        if mail.supports(MailAction::SetReadState) {
            commands.push(self.read_state_command(message_id, cx));
        }
        if mail.supports(MailAction::SetFollowUpFlagStatus) {
            commands.push(self.follow_up_flag_command(message_id, cx));
        }
        (!commands.is_empty()).then(|| {
            CommandBar::new("message-actions", "Message actions")
                .overflow(layout.overflow)
                .commands(commands)
        })
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
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(detail);
        #[cfg(test)]
        let reader = reader.test_support();
        reader.into_any_element()
    }
}
