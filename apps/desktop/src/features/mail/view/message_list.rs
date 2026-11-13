use std::rc::Rc;

use einfach_mail_application::MailWorkflowError;
use einfach_mail_domain::{FolderReference, FollowUpFlagStatus, MailMessage, ReadState};
use gpui_kit::base::v_virtual_list;
use gpui_kit::component::{
    ActiveTheme as _, ColorTokens, Icon, Sizable as _, avatar::Avatar, h_flex,
    resizable::ResizableState, v_flex,
};
#[cfg(test)]
use gpui_kit::test::TestSupportExt as _;
use gpui_kit::{
    AnyElement, App, Context, Div, Entity, FontWeight, IntoElement, Role, WeakEntity, Window, div,
    prelude::*, rems,
};

use super::day_groups::{day_label, received_local, time_label};
use super::{
    MESSAGE_LIST_CONTEXT, MESSAGE_ROW_HEIGHT, MailView, MessageListState, mail_error_message,
    message_sender, message_sender_name,
};
use crate::components::{AppIcon, GroupHeader, LocalClock, SetPosition, list_option};

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum MessageListLayout {
    Split,
    Compact,
}

impl MailView {
    pub(super) fn render_message_list(
        &self,
        colors: &ColorTokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
        message_list.child(div().flex_1().min_h_0().flex().flex_col().child(content))
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
                        range
                            .filter_map(|index| {
                                view.render_message_item(index, &folder_reference, cx)
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

impl MailView {
    /// One virtual list item: the option for the message at `index`, under its day header when the
    /// message starts a day group.
    fn render_message_item(
        &self,
        index: usize,
        folder: &FolderReference,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let message = self.messages.get(index)?;
        let day = self.message_days.get(index)?;
        let message_id = message.id().clone();
        let id = message_id.as_str().to_owned();
        let summary = MessageRowSummary::new(message, &self.clock);
        let selected = self.selected_message_id() == Some(&message_id);
        let update_in_progress = self.message_update_in_progress();
        let accessible_label = if update_in_progress {
            format!("{}, mail action pending", summary.accessible_label())
        } else {
            summary.accessible_label()
        };
        let expected_folder = folder.clone();
        let unread_dot = summary.unread.then(|| render_unread_dot(&id, cx));
        let option = list_option(
            format!("message-option-{id}"),
            SetPosition::new(index, self.messages.len()),
            selected,
            cx,
        )
        .debug_selector({
            let id = id.clone();
            move || format!("message-row-{id}")
        })
        .relative()
        .h(MESSAGE_ROW_HEIGHT)
        .px_4()
        .aria_label(accessible_label)
        .aria_description(summary.accessible_details())
        .children(unread_dot)
        .child(render_message_row_content(&summary, &id, cx))
        .when(!update_in_progress, |option| {
            option.on_click(cx.listener(move |view, _, window, cx| {
                view.select_message_from_option(message_id.clone(), &expected_folder, window, cx);
            }))
        });
        #[cfg(test)]
        let option = option.test_support();
        let header = day.starts_group().then(|| {
            GroupHeader::new(
                format!("message-day-header-{id}"),
                day_label(day.date(), self.clock.today()),
            )
        });
        Some(
            v_flex()
                .w_full()
                .children(header)
                .child(div().w_full().px_1().child(option)),
        )
    }
}

/// What a message row shows, derived from one cached message.
pub(super) struct MessageRowSummary {
    sender_name: String,
    sender: String,
    subject: String,
    day_label: String,
    time_label: String,
    unread: bool,
    flagged: bool,
}

impl MessageRowSummary {
    pub(super) fn new(message: &MailMessage, clock: &LocalClock) -> Self {
        let metadata = message.metadata();
        let received = received_local(message, clock);
        Self {
            sender_name: message_sender_name(message),
            sender: message_sender(message),
            subject: metadata.subject().unwrap_or("(No subject)").to_owned(),
            day_label: day_label(received.date(), clock.today()),
            time_label: time_label(received),
            unread: metadata.read_state() == ReadState::Unread,
            flagged: metadata.follow_up_flag_status() == Some(FollowUpFlagStatus::Flagged),
        }
    }

    /// The option's name: the sender with the address, then the subject.
    pub(super) fn accessible_label(&self) -> String {
        format!("{}. {}", self.sender, self.subject)
    }

    /// The option's description: whether it is unread, when it arrived, and whether it is flagged.
    /// It names the day as well as the time, because the day header is not part of the option.
    pub(super) fn accessible_details(&self) -> String {
        let received = format!("{}, {}", self.day_label, self.time_label);
        let mut details = Vec::new();
        if self.unread {
            details.push("Unread");
        }
        details.push(&received);
        if self.flagged {
            details.push("Flagged");
        }
        details.join(". ")
    }

    /// Unread rows are semibold, so they stand out without relying on color.
    pub(super) fn text_weight(&self) -> FontWeight {
        if self.unread {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        }
    }
}

/// The accent dot that marks an unread row, in the row's leading padding.
fn render_unread_dot(id: &str, cx: &App) -> Div {
    let selector = format!("message-unread-dot-{id}");
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .left(rems(0.25))
        .flex()
        .items_center()
        .child(
            div()
                .debug_selector(move || selector)
                .size(rems(0.5))
                .rounded_full()
                .bg(cx.theme().primary),
        )
}

/// The row anatomy: an avatar, then the sender and time on the first line and the subject and
/// the flag on the second. Both lines end at the same trailing edge.
fn render_message_row_content(summary: &MessageRowSummary, id: &str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let weight = summary.text_weight();
    let avatar_selector = format!("message-avatar-{id}");
    let time_selector = format!("message-time-{id}");
    let flag_selector = format!("message-flag-{id}");
    let time_color = if summary.unread {
        theme.primary
    } else {
        theme.muted_foreground
    };
    let flag = summary.flagged.then(|| {
        div()
            .debug_selector(move || flag_selector)
            .flex_shrink_0()
            .child(Icon::new(AppIcon::Flag).size_4().text_color(theme.warning))
    });
    h_flex()
        .size_full()
        .items_center()
        .gap_3()
        .child(
            div()
                .debug_selector(move || avatar_selector)
                .flex_shrink_0()
                .child(Avatar::new().name(summary.sender_name.clone()).small()),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .font_weight(weight)
                                .child(summary.sender_name.clone()),
                        )
                        .child(
                            div()
                                .debug_selector(move || time_selector)
                                .flex_shrink_0()
                                .text_xs()
                                .font_weight(weight)
                                .text_color(time_color)
                                .child(summary.time_label.clone()),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .font_weight(weight)
                                .child(summary.subject.clone()),
                        )
                        .children(flag),
                ),
        )
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
