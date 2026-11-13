use std::sync::Arc;

use einfach_mail_domain::{FolderReference, MailAction, MailFolder};
use gpui_kit::component::{
    ActiveTheme, ColorTokens, Disableable, Icon, StyledExt,
    button::{Button, ButtonVariants},
    scroll::ScrollableElement,
};
#[cfg(test)]
use gpui_kit::test::TestSupportExt as _;
use gpui_kit::{
    AnyElement, AsyncApp, Context, Entity, IntoElement, Render, Role, Subscription, Window, div,
    prelude::*,
};

use crate::components::{AppIcon, NavItem};

use super::{
    CompactMailPane, FollowUpFlagUpdateStatus, MailView, MessageDetailState, MessageListState,
    MessageMoveStatus, ReadStateUpdateStatus, SyncStatus, WellKnownFolderActionStatus,
    load_folder_snapshot,
};

pub(crate) struct MailFolderNavigation {
    mail_view: Entity<MailView>,
    _subscription: Subscription,
}

impl MailFolderNavigation {
    pub(crate) fn new(mail_view: Entity<MailView>, cx: &mut Context<Self>) -> Self {
        let _subscription = cx.observe(&mail_view, |_, _, cx| cx.notify());
        Self {
            mail_view,
            _subscription,
        }
    }
}

impl MailView {
    pub(super) fn select_folder(&mut self, folder: FolderReference, cx: &mut Context<Self>) {
        if self.message_update_in_progress() {
            return;
        }
        self.compact_pane = CompactMailPane::MessageList;
        if self.selected_folder.as_ref() == Some(&folder) {
            cx.notify();
            return;
        }
        self.message_move_status = MessageMoveStatus::Idle;
        self.well_known_folder_action_status = WellKnownFolderActionStatus::Idle;
        self.selected_folder = Some(folder.clone());
        self.selected_folder_write_sync_required = true;
        self.sync_state = None;
        self.message_list_state = MessageListState::Loading;
        self.message_detail_state = MessageDetailState::Empty;
        self.read_state_update = ReadStateUpdateStatus::Idle;
        self.follow_up_flag_update = FollowUpFlagUpdateStatus::Idle;
        self.set_messages(Vec::new());
        let workflow = Arc::clone(&self.workflow);
        self.message_task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let snapshot = load_folder_snapshot(workflow.as_ref(), &folder).await;
            let _ = this.update(cx, |view, cx| {
                if view.selected_folder.as_ref() != Some(&folder) {
                    return;
                }
                match snapshot {
                    Ok(snapshot) => {
                        view.set_messages(snapshot.messages);
                        view.sync_state = snapshot.sync_state;
                        view.selected_folder_write_sync_required =
                            snapshot.folder_write_sync_required;
                        view.message_list_state = MessageListState::Ready;
                        view.message_detail_state = MessageDetailState::Empty;
                    }
                    Err(error) => {
                        view.message_list_state = MessageListState::Failed(error);
                    }
                }
                view.message_task = None;
                cx.notify();
            });
        }));
        cx.notify();
    }
}

impl Render for MailFolderNavigation {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.folder_navigation_state(cx);
        let colors = cx.theme().semantic_tokens().colors;
        let folder_list = self.render_folder_list(&state, colors);
        self.render_folder_navigation(&state, folder_list, colors)
    }
}

struct FolderNavigationState {
    folders: Vec<MailFolder>,
    selected_folder: Option<FolderReference>,
    folder_actions_disabled: bool,
    empty_state_label: &'static str,
    new_mail: Option<NewMailControl>,
}

/// The "New mail" button, present only when the account can send.
struct NewMailControl {
    label: &'static str,
    disabled: bool,
}

impl MailFolderNavigation {
    fn folder_navigation_state(&self, cx: &mut Context<Self>) -> FolderNavigationState {
        self.mail_view
            .read_with(cx, |mail_view, _| FolderNavigationState {
                folders: mail_view.folders.clone(),
                selected_folder: mail_view.selected_folder.clone(),
                folder_actions_disabled: mail_view.message_update_in_progress(),
                empty_state_label: match &mail_view.sync_status {
                    SyncStatus::LoadingCache | SyncStatus::Syncing | SyncStatus::Rebuilding => {
                        "Loading folders…"
                    }
                    SyncStatus::Failed(_) => "Folders unavailable.",
                    SyncStatus::Idle | SyncStatus::UpToDate => "No visible folders.",
                },
                new_mail: mail_view
                    .profile
                    .capabilities()
                    .mail()
                    .supports(MailAction::Send)
                    .then(|| NewMailControl {
                        label: if matches!(&mail_view.pending_send, Ok(Some(_))) {
                            "Resolve send"
                        } else {
                            "New mail"
                        },
                        disabled: mail_view.sync_status.is_sync_active(),
                    }),
            })
    }

    fn render_new_mail_button(&self, control: &NewMailControl) -> impl IntoElement {
        let mail_view = self.mail_view.downgrade();
        Button::new("new-mail-message")
            .debug_selector(|| "new-mail-message".to_owned())
            .primary()
            .icon(Icon::new(AppIcon::NewMail))
            .label(control.label)
            .w_full()
            .disabled(control.disabled)
            .on_click(move |_, window, cx| {
                if let Some(mail_view) = mail_view.upgrade() {
                    mail_view.update(cx, |view, cx| view.open_composer(window, cx));
                }
            })
    }

    fn render_folder_list(&self, state: &FolderNavigationState, colors: ColorTokens) -> AnyElement {
        let mut folder_list = div()
            .id("mail-folder-list")
            .flex()
            .flex_col()
            .gap_1()
            .overflow_y_scrollbar();
        if state.folders.is_empty() {
            folder_list = folder_list.child(self.render_empty_folder_status(state, colors));
        }
        for folder in &state.folders {
            folder_list = folder_list.child(self.render_folder_item(folder, state));
        }
        folder_list.flex_1().min_h_0().into_any_element()
    }

    fn render_empty_folder_status(
        &self,
        state: &FolderNavigationState,
        colors: ColorTokens,
    ) -> impl IntoElement {
        div()
            .id("mail-folder-status")
            .role(Role::Status)
            .px_3()
            .py_2()
            .text_sm()
            .text_color(colors.muted_foreground)
            .child(state.empty_state_label)
    }

    fn render_folder_item(
        &self,
        folder: &MailFolder,
        state: &FolderNavigationState,
    ) -> impl IntoElement {
        let reference = folder.reference().clone();
        let selector = format!("folder:{}", reference.folder_id().as_str());
        let selected = state.selected_folder.as_ref() == Some(&reference);
        let mail_view = self.mail_view.downgrade();
        NavItem::new(selector, folder.display_name().to_owned(), AppIcon::Folder)
            .selected(selected)
            .unavailable(
                state
                    .folder_actions_disabled
                    .then_some("Wait for the current mail update to finish."),
            )
            .on_click(move |_, _, cx| {
                if let Some(mail_view) = mail_view.upgrade() {
                    mail_view.update(cx, |view, cx| {
                        view.select_folder(reference.clone(), cx);
                    });
                }
            })
    }

    fn render_folder_navigation(
        &self,
        state: &FolderNavigationState,
        folder_list: AnyElement,
        colors: ColorTokens,
    ) -> AnyElement {
        let folders = div()
            .id("mail-folders")
            .role(Role::Group)
            .aria_label("Mail folders")
            .w_full()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .px_3()
                    .text_xs()
                    .font_semibold()
                    .text_color(colors.muted_foreground)
                    .child("Folders"),
            )
            .child(folder_list);
        #[cfg(test)]
        let folders = folders.test_support();
        let navigation = div()
            .id("mail-folder-navigation")
            .w_full()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_3()
            .px_2()
            .py_2()
            .when_some(state.new_mail.as_ref(), |navigation, control| {
                navigation.child(self.render_new_mail_button(control))
            })
            .child(folders);
        #[cfg(test)]
        let navigation = navigation.test_support();
        navigation.into_any_element()
    }
}
