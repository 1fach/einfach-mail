use gpui_kit::component::{
    ActiveTheme, ColorTokens, Disableable, StyledExt, avatar::Avatar, button::Button,
    scroll::ScrollableElement,
};
#[cfg(test)]
use gpui_kit::test::TestSupportExt as _;
use gpui_kit::{Context, IntoElement, Render, Role, Window, div, prelude::*, rems};

use crate::components::{NAVIGATION_PANE_WIDTH, NavItem};
use crate::features::{
    AppearancePreference, SettingsViewState, account_cache_open_error_message, render_settings,
};
use einfach_mail_application::AccountCacheOpenError;

use super::Workspace;
use super::state::{WorkspaceContent, WorkspaceSection};

impl<P: einfach_mail_application::AccountProvider + 'static> Workspace<P> {
    fn render_navigation(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let navigation = div()
            .id("workspace-navigation")
            .role(Role::Navigation)
            .aria_label("Main navigation")
            .w(NAVIGATION_PANE_WIDTH)
            .h_full()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .bg(self
                .navigation_surface_appearance
                .navigation_background(cx.theme().tokens.sidebar.into()))
            .text_color(cx.theme().sidebar_foreground)
            .border_r_1()
            .border_color(cx.theme().sidebar_border);
        #[cfg(test)]
        let navigation = navigation.test_support();
        let navigation = navigation
            .when_some(
                self.account_view.read(cx).navigation_profile(),
                |navigation, profile| {
                    navigation.child(Self::render_active_account_identity(&profile, cx))
                },
            )
            .child(self.render_contextual_navigation());
        navigation.child(self.render_destination_switcher(cx))
    }

    fn render_contextual_navigation(&self) -> gpui_kit::AnyElement {
        match &self.content {
            WorkspaceContent::Mail {
                folder_navigation, ..
            } => folder_navigation.clone().into_any_element(),
            _ => div().flex_1().into_any_element(),
        }
    }

    fn render_active_account_identity(
        profile: &einfach_mail_domain::AccountProfile,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().semantic_tokens().colors;
        let identity = div()
            .id("active-account-identity")
            .role(Role::Group)
            .aria_label(format!("Active account: {}", profile.label()))
            .w_full()
            .flex()
            .items_center()
            .gap_3()
            .px_5()
            .py_3()
            .child(Avatar::new().name(profile.label().to_owned()).size_8())
            .child(Self::render_active_account_details(profile, colors));
        #[cfg(test)]
        let identity = identity.test_support();
        identity
    }

    fn render_active_account_details(
        profile: &einfach_mail_domain::AccountProfile,
        colors: ColorTokens,
    ) -> impl IntoElement {
        let mailbox_address = profile
            .mailbox()
            .map(|mailbox| mailbox.address().to_owned())
            .filter(|address| !profile.label().eq_ignore_ascii_case(address));
        div()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .w_full()
                    .text_sm()
                    .font_semibold()
                    .truncate()
                    .child(profile.label().to_owned()),
            )
            .when_some(mailbox_address, |details, address| {
                details.child(
                    div()
                        .w_full()
                        .text_xs()
                        .text_color(colors.muted_foreground)
                        .truncate()
                        .child(address),
                )
            })
    }

    fn render_destination_switcher(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let switcher = div()
            .id("workspace-destination-switcher")
            .role(Role::Group)
            .aria_label("Destinations")
            .flex()
            .gap_1()
            .p_2()
            .border_t_1()
            .border_color(cx.theme().sidebar_border);
        #[cfg(test)]
        let switcher = switcher.test_support();
        switcher.children([
            self.render_navigation_item(WorkspaceSection::Mail, cx),
            self.render_navigation_item(WorkspaceSection::Calendar, cx),
            self.render_navigation_item(WorkspaceSection::Accounts, cx),
            self.render_navigation_item(WorkspaceSection::Settings, cx),
        ])
    }

    fn render_navigation_item(&self, section: WorkspaceSection, cx: &mut Context<Self>) -> NavItem {
        let (id, label, icon) = section.navigation_identity();
        NavItem::icon_only(id, label, icon)
            .selected(self.content.selected_section() == section)
            .unavailable(self.navigation_disabled_reason(section, cx))
            .track_focus(self.navigation_focus(section))
            .on_click(cx.listener(move |workspace, _, _, cx| workspace.navigate_to(section, cx)))
    }

    fn render_workspace_content(
        &self,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        match &self.content {
            WorkspaceContent::Mail { view, .. } => {
                div().size_full().child(view.clone()).into_any_element()
            }
            WorkspaceContent::Calendar { view, .. } => {
                div().size_full().child(view.clone()).into_any_element()
            }
            WorkspaceContent::Settings => self.render_settings_workspace_content(colors, cx),
            WorkspaceContent::Accounts | WorkspaceContent::Opening(_) => {
                self.render_account_workspace_content(colors)
            }
        }
    }

    fn render_account_workspace_content(&self, colors: ColorTokens) -> gpui_kit::AnyElement {
        let mut account_content = div().flex().flex_col().child(self.account_view.clone());
        if let Some(error) = self.session_open_error {
            account_content = account_content.child(Self::render_session_open_error(error));
        }
        if let Some(opening) = self.content.opening_kind() {
            account_content = account_content.child(Self::render_opening_progress(opening));
        }
        div()
            .size_full()
            .overflow_y_scrollbar()
            .id("account-scroll-viewport")
            .bg(colors.background)
            .child(
                div()
                    .min_h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_8()
                    .child(account_content),
            )
            .into_any_element()
    }

    fn render_session_open_error(error: AccountCacheOpenError) -> impl IntoElement {
        div()
            .id("account-cache-error")
            .debug_selector(|| "account-cache-error".to_owned())
            .role(Role::Alert)
            .w_full()
            .max_w(rems(48.))
            .text_sm()
            .child(account_cache_open_error_message(error))
    }

    fn render_opening_progress(opening: super::state::OpeningContent) -> impl IntoElement {
        let label = match opening {
            super::state::OpeningContent::Mail => "Opening mail…",
            super::state::OpeningContent::Calendar => "Opening calendar…",
        };
        div().flex().justify_center().child(
            Button::new("account-operation-progress")
                .debug_selector(|| "account-operation-progress".to_owned())
                .label(label)
                .loading(true)
                .disabled(true),
        )
    }

    fn render_settings_workspace_content(
        &self,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let on_change = cx.listener(|workspace, index, window, cx| {
            if let Some(appearance) = AppearancePreference::from_radio_index(*index) {
                workspace.change_appearance(appearance, window, cx);
            }
        });
        div()
            .size_full()
            .child(render_settings(
                SettingsViewState::new(
                    self.appearance_preference,
                    self.appearance_settings_notice,
                    colors,
                    cx.theme().radius,
                ),
                on_change,
            ))
            .into_any_element()
    }
}

impl<P: einfach_mail_application::AccountProvider + 'static> Render for Workspace<P> {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().semantic_tokens().colors;
        div()
            .id("workspace")
            .size_full()
            .flex()
            .child(self.render_navigation(cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .h_full()
                    .overflow_hidden()
                    .child(self.render_workspace_content(colors, cx)),
            )
            .into_any_element()
    }
}
