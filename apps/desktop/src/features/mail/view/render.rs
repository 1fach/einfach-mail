use einfach_mail_domain::MailAction;
use gpui_kit::component::{
    ActiveTheme, ColorTokens, Disableable, StyledExt,
    button::{Button, ButtonVariants},
    resizable::{h_resizable, resizable_panel},
};
#[cfg(test)]
use gpui_kit::test::TestSupportExt as _;
use gpui_kit::{
    AnyElement, Context, IntoElement, Pixels, Rems, Render, Role, Window, div, prelude::*, rems,
};

use super::message_list::{MessageListLayout, record_resized_message_list_width};
use super::{
    CompactMailPane, MAIL_SHELL_NAVIGATION_WIDTH, MAIL_SPLIT_HANDLE_RESERVE,
    MESSAGE_LIST_MAX_WIDTH, MESSAGE_LIST_MIN_WIDTH, MESSAGE_READER_CONTEXT,
    MESSAGE_READER_MIN_WIDTH, MailView, SyncStatus,
};

const MAIL_HEADER_HEIGHT: Rems = rems(3.625);

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
                    .child(self.render_message_list(MessageListLayout::Split, &colors, cx)),
            )
            .child(
                resizable_panel()
                    .size_range(MESSAGE_READER_MIN_WIDTH.to_pixels(rem_size)..Pixels::MAX)
                    .child(self.render_message_detail(&colors, cx)),
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
                .child(self.render_message_list(MessageListLayout::Compact, &colors, cx))
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
            .child(self.render_message_detail(&colors, cx))
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
