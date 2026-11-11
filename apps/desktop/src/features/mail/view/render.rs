use std::rc::Rc;

use super::super::body::render_message_body;
use einfach_mail_application::WellKnownMailFolder;
use einfach_mail_domain::MailAction;
use gpui_kit::base::v_virtual_list;
use gpui_kit::component::{
    ActiveTheme, ColorTokens, Disableable, StyledExt,
    button::{Button, ButtonVariants},
    resizable::{ResizableState, h_resizable, resizable_panel},
};
#[cfg(test)]
use gpui_kit::test::TestSupportExt as _;
use gpui_kit::{
    AnyElement, App, Context, Entity, IntoElement, Pixels, Rems, Render, Role, WeakEntity, Window,
    div, prelude::*, rems,
};

use super::{
    CompactMailPane, MAIL_SHELL_NAVIGATION_WIDTH, MAIL_SPLIT_HANDLE_RESERVE, MESSAGE_LIST_CONTEXT,
    MESSAGE_LIST_MAX_WIDTH, MESSAGE_LIST_MIN_WIDTH, MESSAGE_READER_CONTEXT,
    MESSAGE_READER_MIN_WIDTH, MESSAGE_ROW_HEIGHT, MailView, MessageDetailState, MessageListState,
    MessageListWidthChange, SyncStatus, empty_message_detail_copy, mail_error_message,
    message_sender,
};

const MAIL_HEADER_HEIGHT: Rems = rems(3.625);

#[derive(Clone, Copy, Eq, PartialEq)]
enum MessageListLayout {
    Split,
    Compact,
}

impl MailView {
    fn render_header(&self, colors: ColorTokens, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.sync_status_label();
        let status_role = if matches!(self.sync_status, SyncStatus::Failed(_)) {
            Role::Alert
        } else {
            Role::Status
        };
        let sync_status = div()
            .id("mail-sync-status")
            .text_sm()
            .text_color(colors.muted_foreground)
            .role(status_role)
            .aria_label(status.clone())
            .child(status);
        #[cfg(test)]
        let sync_status = sync_status
            .debug_selector(|| "mail-sync-status".to_owned())
            .test_support();

        let mut header_actions = div().flex().items_center().gap_2();
        if self
            .profile
            .capabilities()
            .mail()
            .supports(MailAction::Send)
        {
            header_actions = header_actions.child(
                Button::new("new-mail-message")
                    .debug_selector(|| "new-mail-message".to_owned())
                    .primary()
                    .label(if matches!(&self.pending_send, Ok(Some(_))) {
                        "Resolve send"
                    } else {
                        "Compose"
                    })
                    .disabled(self.sync_status.is_sync_active())
                    .on_click(cx.listener(|view, _, window, cx| {
                        view.open_composer(window, cx);
                    })),
            );
        }

        div()
            .w_full()
            .h(MAIL_HEADER_HEIGHT)
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .px_4()
            .border_b_1()
            .border_color(colors.border)
            .child({
                let mut title = div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(div().text_lg().font_bold().child("Mail"));
                if self.profile.capabilities().mail().is_read_only() {
                    title = title.child(
                        div()
                            .text_sm()
                            .text_color(colors.muted_foreground)
                            .child("Read-only"),
                    );
                }
                title.child(sync_status)
            })
            .child(
                header_actions.child(
                    Button::new("sync-mail")
                        .ghost()
                        .label("Sync")
                        .loading(self.sync_status.is_sync_active())
                        .disabled(
                            self.sync_status.is_sync_active() || self.message_update_in_progress(),
                        )
                        .on_click(cx.listener(|view, _, _, cx| view.refresh(cx))),
                ),
            )
    }

    fn render_message_list(
        &self,
        layout: MessageListLayout,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let header = self.render_message_list_header(layout, cx);
        let content = self.render_message_list_content(colors, cx);
        let message_list = div()
            .id("mail-message-list")
            .role(Role::Group)
            .aria_label("Message list")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(colors.border);
        #[cfg(test)]
        let message_list = message_list.test_support();
        message_list
            .child(header)
            .child(div().flex_1().min_h_0().flex().flex_col().child(content))
    }

    fn render_resizable_message_panes(
        &self,
        rem_size: Pixels,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let resizable_state = self.pane_layout.resizable_state.clone();
        let mail_view = cx.entity().downgrade();
        h_resizable("mail-message-reader-layout")
            .with_state(&resizable_state)
            .on_resize(move |state, window, cx| {
                record_resized_message_list_width(state, &mail_view, window, cx);
            })
            .child(
                resizable_panel()
                    .size(self.pane_layout.message_list_width.to_pixels(rem_size))
                    .size_range(
                        MESSAGE_LIST_MIN_WIDTH.to_pixels(rem_size)
                            ..MESSAGE_LIST_MAX_WIDTH.to_pixels(rem_size),
                    )
                    .fixed()
                    .child(self.render_message_list(MessageListLayout::Split, colors, cx)),
            )
            .child(
                resizable_panel()
                    .size_range(MESSAGE_READER_MIN_WIDTH.to_pixels(rem_size)..Pixels::MAX)
                    .child(self.render_message_detail(colors, cx)),
            )
            .into_any_element()
    }

    fn render_message_panes(
        &mut self,
        window: &mut Window,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rem_size = window.rem_size();
        self.pane_layout.sync_rem_size(rem_size, cx);
        self.update_message_row_sizes(rem_size);

        if self.uses_compact_mail_layout(window, rem_size) {
            self.render_compact_message_pane(colors, cx)
        } else {
            self.render_resizable_message_panes(rem_size, colors, cx)
        }
    }

    fn render_compact_message_pane(
        &self,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.compact_pane {
            CompactMailPane::MessageList => div()
                .id("mail-compact-message-list")
                .size_full()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .child(self.render_message_list(MessageListLayout::Compact, colors, cx))
                .into_any_element(),
            CompactMailPane::MessageReader => self.render_compact_message_reader(colors, cx),
        }
    }

    fn uses_compact_mail_layout(&self, window: &Window, rem_size: Pixels) -> bool {
        let wide_minimum_width = MAIL_SHELL_NAVIGATION_WIDTH.to_pixels(rem_size)
            + self.pane_layout.message_list_width.0.to_pixels(rem_size)
            + MESSAGE_READER_MIN_WIDTH.to_pixels(rem_size)
            + MAIL_SPLIT_HANDLE_RESERVE.to_pixels(rem_size);
        window.bounds().size.width < wide_minimum_width
    }

    fn render_compact_message_reader(
        &self,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id("mail-compact-reader")
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(colors.background)
            .key_context(MESSAGE_READER_CONTEXT)
            .on_action(cx.listener(Self::on_return_to_message_list))
            .child(self.render_compact_reader_toolbar(colors, cx))
            .child(self.render_message_detail(colors, cx))
            .into_any_element()
    }

    fn render_compact_reader_toolbar(
        &self,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id("mail-compact-reader-toolbar")
            .w_full()
            .flex()
            .items_center()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(colors.border)
            .child(self.render_back_to_messages_button(cx))
    }

    fn render_back_to_messages_button(&self, cx: &mut Context<Self>) -> Button {
        Button::new("mail-reader-back")
            .ghost()
            .label("Back to messages")
            .track_focus(&self.message_list_focus)
            .on_click(cx.listener(|view, _, window, cx| {
                view.return_to_message_list(window, cx);
            }))
    }

    fn render_message_list_header(
        &self,
        layout: MessageListLayout,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id("mail-message-list-header")
            .role(Role::Group)
            .aria_label("Message list controls")
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .px_3()
            .py_2()
            .child(div().text_sm().font_semibold().child("Messages"))
            .when(layout == MessageListLayout::Split, |header| {
                header.child(self.render_message_list_width_controls(cx))
            })
            .into_any_element()
    }

    fn render_message_list_width_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let narrower = self.render_message_list_width_button(MessageListWidthChange::Narrower, cx);
        let wider = self.render_message_list_width_button(MessageListWidthChange::Wider, cx);
        div()
            .tab_group()
            .flex()
            .items_center()
            .gap_1()
            .child(narrower)
            .child(wider)
            .into_any_element()
    }

    fn render_message_list_width_button(
        &self,
        change: MessageListWidthChange,
        cx: &mut Context<Self>,
    ) -> Button {
        Button::new(change.button_id())
            .ghost()
            .label(change.button_label())
            .disabled(self.pane_layout.message_list_width.is_disabled_for(change))
            .on_click(cx.listener(move |view, _, window, cx| {
                view.pane_layout.resize(change, window, cx);
            }))
    }

    fn render_message_list_content(
        &self,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match &self.message_list_state {
            MessageListState::Ready => {}
            MessageListState::Loading => {
                let loading = div()
                    .min_h_0()
                    .p_4()
                    .id("mail-list-loading")
                    .role(Role::Status)
                    .child("Loading messages…");
                #[cfg(test)]
                let loading = loading
                    .debug_selector(|| "mail-list-loading".to_owned())
                    .test_support();
                return loading.into_any_element();
            }
            MessageListState::Failed(error) => {
                if self.selected_folder.is_none() {
                    return div().min_h_0().into_any_element();
                }
                let list_error = div()
                    .min_h_0()
                    .p_4()
                    .id("mail-list-error")
                    .role(Role::Alert)
                    .aria_label(mail_error_message(*error))
                    .child(mail_error_message(*error));
                #[cfg(test)]
                let list_error = list_error
                    .debug_selector(|| "mail-list-error".to_owned())
                    .test_support();
                return list_error.into_any_element();
            }
        }
        let Some(folder) = self.selected_folder.as_ref() else {
            let empty = div()
                .id("mail-list-empty-folders")
                .min_h_0()
                .p_4()
                .text_color(colors.muted_foreground)
                .child("No visible mail folders were found.");
            #[cfg(test)]
            let empty = empty
                .debug_selector(|| "mail-list-empty-folders".to_owned())
                .test_support();
            return empty.into_any_element();
        };
        if self.messages.is_empty() {
            return div()
                .min_h_0()
                .p_4()
                .text_color(colors.muted_foreground)
                .child("No messages in this folder.")
                .into_any_element();
        }

        let folder_reference = folder.clone();
        let sizes = Rc::clone(&self.message_row_sizes);
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
            .on_action(cx.listener(Self::on_open_selected_message))
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
                                let selected = view.selected_message_id() == Some(&message_id);
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
                                                div().w_full().text_sm().font_bold().child(sender),
                                            )
                                            .child(div().w_full().text_sm().child(subject)),
                                    )
                                    .when(!view.message_update_in_progress(), |option| {
                                        option.on_click(cx.listener(move |view, _, window, cx| {
                                            view.select_message_from_option(
                                                message_id.clone(),
                                                &expected_folder,
                                                window,
                                                cx,
                                            );
                                        }))
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
            .into_any_element()
    }

    fn render_message_detail(&self, colors: ColorTokens, cx: &mut Context<Self>) -> AnyElement {
        let detail = match &self.message_detail_state {
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
            MessageDetailState::Loading(_) => div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .items_center()
                .justify_center()
                .text_color(colors.muted_foreground)
                .child("Loading message…")
                .into_any_element(),
            MessageDetailState::Failed { error, .. } => {
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
                                        let mail = self.profile.capabilities().mail();
                                        if mail.is_read_only() {
                                            div().into_any_element()
                                        } else {
                                            let mut actions = div().flex().items_center().gap_2();
                                            if mail.supports(MailAction::SetReadState) {
                                                actions = actions.child(
                                                    self.render_read_state_action(message_id, cx),
                                                );
                                            }
                                            if mail.supports(MailAction::SetFollowUpFlagStatus) {
                                                actions = actions.child(
                                                    self.render_follow_up_flag_action(
                                                        message_id, cx,
                                                    ),
                                                );
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
                                                actions = actions.child(
                                                    self.render_reply_action(message_id, cx),
                                                );
                                            }
                                            if mail.supports(MailAction::Delete) {
                                                actions = actions.child(
                                                    self.render_well_known_folder_action(
                                                        WellKnownMailFolder::DeletedItems,
                                                        message_id,
                                                        cx,
                                                    ),
                                                );
                                            }
                                            #[cfg(test)]
                                            let actions =
                                                actions.id("message-actions").test_support();
                                            actions.into_any_element()
                                        }
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
        };
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

fn record_resized_message_list_width(
    state: &Entity<ResizableState>,
    mail_view: &WeakEntity<MailView>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(width) = state.read(cx).sizes().first().copied() else {
        return;
    };
    let Some(mail_view) = mail_view.upgrade() else {
        return;
    };
    mail_view.update(cx, |view, cx| {
        view.pane_layout
            .record_dragged_width(width, window.rem_size());
        cx.notify();
    });
}

impl Render for MailView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().semantic_tokens().colors;
        let panes = self.render_message_panes(window, colors, cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.background)
            .text_color(colors.foreground)
            .child(self.render_header(colors, cx))
            .child(div().flex_1().min_h_0().min_w_0().child(panes))
    }
}
