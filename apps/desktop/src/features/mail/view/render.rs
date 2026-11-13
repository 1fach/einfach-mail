use gpui_kit::component::{
    ActiveTheme, ColorTokens, Disableable, Icon,
    button::{Button, ButtonVariants},
    resizable::{h_resizable, resizable_panel},
};
#[cfg(test)]
use gpui_kit::test::TestSupportExt as _;
use gpui_kit::{AnyElement, Context, IntoElement, Pixels, Render, Role, Window, div, prelude::*};

use crate::components::{AppIcon, NAVIGATION_PANE_WIDTH, PaneHeader};

use super::message_list::{MessageListLayout, record_resized_message_list_width};
use super::message_reader::ReaderLayout;
use super::{
    CompactMailPane, MAIL_SPLIT_HANDLE_RESERVE, MESSAGE_LIST_MAX_WIDTH, MESSAGE_LIST_MIN_WIDTH,
    MESSAGE_READER_CONTEXT, MESSAGE_READER_MIN_WIDTH, MailView, MessageListWidthChange, SyncStatus,
};

impl MailView {
    fn render_header(
        &self,
        layout: MessageListLayout,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
            .truncate()
            .role(status_role)
            .aria_label(status.clone())
            .child(status);
        #[cfg(test)]
        let sync_status = sync_status
            .debug_selector(|| "mail-sync-status".to_owned())
            .test_support();

        let sync_button = Button::new("sync-mail")
            .ghost()
            .icon(Icon::new(AppIcon::Sync))
            .accessibility_label("Sync")
            .tooltip("Sync")
            .loading(self.sync_status.is_sync_active())
            .disabled(self.sync_status.is_sync_active() || self.message_update_in_progress())
            .on_click(cx.listener(|view, _, _, cx| view.refresh(cx)));
        #[cfg(test)]
        let sync_button = sync_button.debug_selector(|| "sync-mail".to_owned());

        PaneHeader::new("mail-folder-title", self.selected_folder_name().to_owned())
            .when(
                self.profile.capabilities().mail().is_read_only(),
                |header| header.tag("Read-only"),
            )
            .status(sync_status)
            .action(sync_button)
            .when(layout == MessageListLayout::Split, |header| {
                header
                    .action(
                        self.render_message_list_width_button(MessageListWidthChange::Narrower, cx),
                    )
                    .action(
                        self.render_message_list_width_button(MessageListWidthChange::Wider, cx),
                    )
            })
    }

    fn render_message_list_width_button(
        &self,
        change: MessageListWidthChange,
        cx: &mut Context<Self>,
    ) -> Button {
        let icon = match change {
            MessageListWidthChange::Narrower => AppIcon::NarrowList,
            MessageListWidthChange::Wider => AppIcon::WidenList,
        };
        Button::new(change.button_id())
            .ghost()
            .icon(Icon::new(icon))
            .accessibility_label(change.button_label())
            .tooltip(change.button_label())
            .disabled(self.pane_layout.message_list_width.is_disabled_for(change))
            .on_click(cx.listener(move |view, _, window, cx| {
                view.pane_layout.resize(change, window, cx);
            }))
    }

    fn render_resizable_message_panes(
        &self,
        rem_size: Pixels,
        reader: ReaderLayout,
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
                    .child(self.render_message_list(&colors, cx)),
            )
            .child(
                resizable_panel()
                    .size_range(MESSAGE_READER_MIN_WIDTH.to_pixels(rem_size)..Pixels::MAX)
                    .child(self.render_message_detail(&colors, reader, cx)),
            )
            .into_any_element()
    }

    fn render_message_panes(
        &mut self,
        layout: MessageListLayout,
        reader: ReaderLayout,
        rem_size: Pixels,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.pane_layout.sync_rem_size(rem_size, cx);
        self.update_message_row_sizes(rem_size);

        match layout {
            MessageListLayout::Compact => self.render_compact_message_pane(reader, colors, cx),
            MessageListLayout::Split => {
                self.render_resizable_message_panes(rem_size, reader, colors, cx)
            }
        }
    }

    fn render_compact_message_pane(
        &self,
        reader: ReaderLayout,
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
                .child(self.render_message_list(&colors, cx))
                .into_any_element(),
            CompactMailPane::MessageReader => {
                self.render_compact_message_reader(reader, colors, cx)
            }
        }
    }

    fn message_list_layout(&self, window: &Window, rem_size: Pixels) -> MessageListLayout {
        if self.uses_compact_mail_layout(window, rem_size) {
            MessageListLayout::Compact
        } else {
            MessageListLayout::Split
        }
    }

    fn uses_compact_mail_layout(&self, window: &Window, rem_size: Pixels) -> bool {
        let wide_minimum_width = NAVIGATION_PANE_WIDTH.to_pixels(rem_size)
            + self.pane_layout.message_list_width.0.to_pixels(rem_size)
            + MESSAGE_READER_MIN_WIDTH.to_pixels(rem_size)
            + MAIL_SPLIT_HANDLE_RESERVE.to_pixels(rem_size);
        window.bounds().size.width < wide_minimum_width
    }

    fn render_compact_message_reader(
        &self,
        reader: ReaderLayout,
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
            .child(self.render_compact_reader_toolbar(reader, colors, cx))
            .child(self.render_message_detail(&colors, reader, cx))
            .into_any_element()
    }

    fn render_compact_reader_toolbar(
        &self,
        reader: ReaderLayout,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let toolbar = div()
            .id("mail-compact-reader-toolbar")
            .w_full()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(colors.border);
        #[cfg(test)]
        let toolbar = toolbar.test_support();
        toolbar
            .child(self.render_back_to_messages_button(cx))
            .children(self.message_command_bar(reader, cx))
    }

    fn render_back_to_messages_button(&self, cx: &mut Context<Self>) -> Button {
        Button::new("mail-reader-back")
            .ghost()
            .icon(Icon::new(AppIcon::Back))
            .accessibility_label("Back to messages")
            .tooltip("Back to messages")
            .track_focus(&self.message_list_focus)
            .on_click(cx.listener(|view, _, window, cx| {
                view.return_to_message_list(window, cx);
            }))
    }
}

impl Render for MailView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().semantic_tokens().colors;
        let rem_size = window.rem_size();
        let layout = self.message_list_layout(window, rem_size);
        let reader = self.reader_layout(
            window,
            rem_size,
            matches!(layout, MessageListLayout::Compact),
        );
        let panes = self.render_message_panes(layout, reader, rem_size, colors, cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.background)
            .text_color(colors.foreground)
            .child(self.render_header(layout, colors, cx))
            .child(div().flex_1().min_h_0().min_w_0().child(panes))
    }
}
