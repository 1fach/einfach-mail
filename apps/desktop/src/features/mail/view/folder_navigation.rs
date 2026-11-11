use einfach_mail_domain::{FolderReference, MailFolder};
use gpui_kit::component::{
    ActiveTheme, ColorTokens, Disableable, Selectable, StyledExt,
    button::{Button, ButtonVariants},
    scroll::ScrollableElement,
};
#[cfg(test)]
use gpui_kit::test::TestSupportExt as _;
use gpui_kit::{
    AnyElement, Context, Entity, IntoElement, Render, Role, Subscription, Window, div, prelude::*,
};

use super::{MailView, SyncStatus};

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

impl Render for MailFolderNavigation {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.folder_navigation_state(cx);
        let colors = cx.theme().semantic_tokens().colors;
        let folder_list = self.render_folder_list(&state, colors);
        self.render_folder_navigation(folder_list, colors)
    }
}

struct FolderNavigationState {
    folders: Vec<MailFolder>,
    selected_folder: Option<FolderReference>,
    folder_actions_disabled: bool,
    empty_state_label: &'static str,
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
            folder_list = folder_list.child(self.render_folder_button(folder, state));
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
            .px_2()
            .py_2()
            .text_sm()
            .text_color(colors.muted_foreground)
            .child(state.empty_state_label)
    }

    fn render_folder_button(
        &self,
        folder: &MailFolder,
        state: &FolderNavigationState,
    ) -> impl IntoElement {
        let reference = folder.reference().clone();
        let selector = format!("folder:{}", reference.folder_id().as_str());
        let label = folder.display_name().to_owned();
        let selected = state.selected_folder.as_ref() == Some(&reference);
        let mail_view = self.mail_view.downgrade();
        Button::new(selector.clone())
            .debug_selector(move || selector.clone())
            .ghost()
            .selected(selected)
            .label(label)
            .w_full()
            .disabled(state.folder_actions_disabled)
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
        folder_list: impl IntoElement,
        colors: ColorTokens,
    ) -> impl IntoElement {
        let navigation = div()
            .id("mail-folder-navigation")
            .role(Role::Group)
            .aria_label("Mail folders")
            .w_full()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_2()
            .px_2()
            .py_3()
            .child(
                div()
                    .px_2()
                    .text_sm()
                    .font_semibold()
                    .text_color(colors.muted_foreground)
                    .child("Folders"),
            )
            .child(folder_list);
        #[cfg(test)]
        let navigation = navigation.test_support();
        navigation
    }
}
