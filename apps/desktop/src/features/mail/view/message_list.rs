use std::rc::Rc;

use einfach_mail_application::MailWorkflowError;
use einfach_mail_domain::FolderReference;
use gpui_kit::base::v_virtual_list;
use gpui_kit::component::{
    ColorTokens, Disableable, StyledExt,
    button::{Button, ButtonVariants},
    resizable::ResizableState,
};
#[cfg(test)]
use gpui_kit::test::TestSupportExt as _;
use gpui_kit::{
    AnyElement, App, Context, Entity, IntoElement, Role, WeakEntity, Window, div, prelude::*,
};

use super::{
    MESSAGE_LIST_CONTEXT, MESSAGE_ROW_HEIGHT, MailView, MessageListState, MessageListWidthChange,
    mail_error_message, message_sender,
};

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum MessageListLayout {
    Split,
    Compact,
}

impl MailView {
    pub(super) fn render_message_list(
        &self,
        layout: MessageListLayout,
        colors: &ColorTokens,
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
        colors: &ColorTokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match &self.message_list_state {
            MessageListState::Ready => self.render_ready_message_list(colors, cx),
            MessageListState::Loading => Self::render_loading_message_list(),
            MessageListState::Failed(error) => self.render_failed_message_list(*error),
        }
    }

    fn render_loading_message_list() -> AnyElement {
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
        loading.into_any_element()
    }

    fn render_failed_message_list(&self, error: MailWorkflowError) -> AnyElement {
        if self.selected_folder.is_none() {
            return div().min_h_0().into_any_element();
        }
        let list_error = div()
            .min_h_0()
            .p_4()
            .id("mail-list-error")
            .role(Role::Alert)
            .aria_label(mail_error_message(error))
            .child(mail_error_message(error));
        #[cfg(test)]
        let list_error = list_error
            .debug_selector(|| "mail-list-error".to_owned())
            .test_support();
        list_error.into_any_element()
    }

    fn render_ready_message_list(
        &self,
        colors: &ColorTokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(folder) = self.selected_folder.as_ref() else {
            return Self::render_empty_folder_list(colors);
        };
        if self.messages.is_empty() {
            return Self::render_empty_folder_messages(colors);
        }
        self.render_cached_message_list(folder, colors, cx)
    }

    fn render_empty_folder_list(colors: &ColorTokens) -> AnyElement {
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
        empty.into_any_element()
    }

    fn render_empty_folder_messages(colors: &ColorTokens) -> AnyElement {
        div()
            .min_h_0()
            .p_4()
            .text_color(colors.muted_foreground)
            .child("No messages in this folder.")
            .into_any_element()
    }

    fn render_cached_message_list(
        &self,
        folder: &FolderReference,
        colors: &ColorTokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let folder_reference = folder.clone();
        let colors = *colors;
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
                                    .child(render_message_row_content(sender, subject))
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
}

fn render_message_row_content(sender: String, subject: String) -> AnyElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().w_full().text_sm().font_bold().child(sender))
        .child(div().w_full().text_sm().child(subject))
        .into_any_element()
}

pub(super) fn record_resized_message_list_width(
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
