use std::sync::Arc;

use crate::components::ListNavigation;
use einfach_mail_domain::{FolderReference, MessageId};
use gpui_kit::{App, AsyncApp, Context, KeyBinding, ScrollStrategy, Window};

use super::{CompactMailPane, MailView, MessageDetailState};

gpui_kit::actions!(
    mail_message_list,
    [
        SelectPreviousMessage,
        SelectNextMessage,
        SelectFirstMessage,
        SelectLastMessage,
        OpenSelectedMessage,
        ReturnToMessageList,
    ]
);

pub(super) const MESSAGE_LIST_CONTEXT: &str = "MailMessageList";
pub(super) const MESSAGE_READER_CONTEXT: &str = "MailMessageReader";

pub(crate) fn bind_message_navigation_keys(cx: &mut App) {
    let context = Some(MESSAGE_LIST_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("up", SelectPreviousMessage, context),
        KeyBinding::new("down", SelectNextMessage, context),
        KeyBinding::new("home", SelectFirstMessage, context),
        KeyBinding::new("end", SelectLastMessage, context),
        KeyBinding::new("enter", OpenSelectedMessage, context),
        KeyBinding::new("escape", ReturnToMessageList, Some(MESSAGE_READER_CONTEXT)),
    ]);
}

impl MailView {
    pub(super) fn select_message(&mut self, message_id: MessageId, cx: &mut Context<Self>) {
        if self.message_update_in_progress() {
            return;
        }
        if !self
            .messages
            .iter()
            .any(|message| message.id() == &message_id)
        {
            return;
        }
        let Some(folder) = self.selected_folder.clone() else {
            return;
        };
        self.message_detail_state = MessageDetailState::Loading(message_id.clone());
        let workflow = Arc::clone(&self.workflow);
        let request_message_id = message_id.clone();
        self.message_task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            // Keep HTML sanitization off GPUI's event-loop executor.
            let opened = cx
                .background_executor()
                .spawn(async move { workflow.open_message(&folder, &request_message_id).await })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.selected_message_id() != Some(&message_id) {
                    return;
                }
                view.message_detail_state = match opened {
                    Ok(message) => MessageDetailState::Opened(Box::new(message)),
                    Err(error) => MessageDetailState::Failed { message_id, error },
                };
                view.message_task = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn navigate_message(&mut self, navigation: ListNavigation, cx: &mut Context<Self>) {
        if self.message_update_in_progress() {
            return;
        }
        let selected_index = self.selected_message_id().and_then(|selected| {
            self.messages
                .iter()
                .position(|message| message.id() == selected)
        });
        let Some(target_index) = navigation.target_index(selected_index, self.messages.len())
        else {
            return;
        };
        let Some(message) = self.messages.get(target_index) else {
            return;
        };
        if self.selected_message_id() == Some(message.id()) {
            return;
        }
        self.message_list_scroll
            .scroll_to_item(target_index, ScrollStrategy::Nearest);
        self.select_message(message.id().clone(), cx);
    }

    pub(super) fn select_message_from_option(
        &mut self,
        message_id: MessageId,
        expected_folder: &FolderReference,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_folder.as_ref() != Some(expected_folder) {
            return;
        }
        self.message_list_focus.focus(window, cx);
        self.compact_pane = CompactMailPane::MessageReader;
        self.select_message(message_id, cx);
    }

    pub(super) fn on_open_selected_message(
        &mut self,
        _: &OpenSelectedMessage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.message_update_in_progress() || self.selected_message_id().is_none() {
            return;
        }
        self.compact_pane = CompactMailPane::MessageReader;
        self.message_list_focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn on_return_to_message_list(
        &mut self,
        _: &ReturnToMessageList,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.return_to_message_list(window, cx);
    }

    pub(super) fn return_to_message_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.compact_pane = CompactMailPane::MessageList;
        self.message_list_focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn on_select_previous_message(
        &mut self,
        _: &SelectPreviousMessage,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_message(ListNavigation::Previous, cx);
    }

    pub(super) fn on_select_next_message(
        &mut self,
        _: &SelectNextMessage,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_message(ListNavigation::Next, cx);
    }

    pub(super) fn on_select_first_message(
        &mut self,
        _: &SelectFirstMessage,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_message(ListNavigation::First, cx);
    }

    pub(super) fn on_select_last_message(
        &mut self,
        _: &SelectLastMessage,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_message(ListNavigation::Last, cx);
    }
}
