use std::sync::Arc;

mod actions;
#[cfg(feature = "ui-preview")]
mod preview_start;
mod render;
mod state;

#[cfg(test)]
mod test_access;

#[cfg(test)]
mod tests;

use einfach_mail_application::{
    AccountCacheOpenError, AccountProvider, AccountSession, AccountSessionFactory,
};
use gpui_kit::{AppContext, Context, Entity, FocusHandle, Subscription, Task, Window};

use crate::features::{
    AccountServices, AccountView, AccountViewEvent, AppearancePreference, AppearanceSettingsNotice,
    AppearanceSettingsStore, MailPermissionRequester, SetupIssue,
};

pub(crate) use state::NavigationSurfaceAppearance;
use state::WorkspaceContent;
#[cfg(test)]
pub(crate) use test_access::{WorkspaceTestContent, WorkspaceTestSection};

pub(crate) struct Workspace<P: AccountProvider + 'static> {
    account_view: Entity<AccountView<P>>,
    account_session_factory: Option<Arc<dyn AccountSessionFactory>>,
    mail_permission_requester: Option<Arc<dyn MailPermissionRequester>>,
    account_session: Option<Arc<dyn AccountSession>>,
    session_open_error: Option<AccountCacheOpenError>,
    navigation_surface_appearance: NavigationSurfaceAppearance,
    content: WorkspaceContent,
    appearance_preference: AppearancePreference,
    appearance_settings_store: Option<AppearanceSettingsStore>,
    appearance_settings_notice: Option<AppearanceSettingsNotice>,
    _appearance_subscription: Option<Subscription>,
    _account_view_subscription: Subscription,
    _account_view_observation: Subscription,
    mail_navigation_focus: FocusHandle,
    calendar_navigation_focus: FocusHandle,
    accounts_navigation_focus: FocusHandle,
    settings_navigation_focus: FocusHandle,
    task: Option<Task<()>>,
    #[cfg(feature = "ui-preview")]
    preview_start: Option<preview_start::PreviewStartStep>,
    #[cfg(feature = "ui-preview")]
    _preview_message_observation: Option<Subscription>,
}

impl<P: AccountProvider + 'static> Workspace<P> {
    pub(crate) fn new(
        services: Result<AccountServices<P>, SetupIssue>,
        cx: &mut Context<Self>,
    ) -> Self {
        let account_view = cx.new(|cx| AccountView::new(services, cx));
        let account_session_factory = account_view.read(cx).session_factory();
        let mail_permission_requester: Option<Arc<dyn MailPermissionRequester>> = account_view
            .read(cx)
            .workflow()
            .map(|workflow| -> Arc<dyn MailPermissionRequester> { workflow });
        let account_view_subscription = cx.subscribe(
            &account_view,
            |workspace, _, event: &AccountViewEvent, cx| workspace.on_account_view_event(event, cx),
        );
        let account_view_observation = cx.observe(&account_view, |_workspace, _, cx| {
            #[cfg(feature = "ui-preview")]
            _workspace.advance_preview_start(cx);
            cx.notify();
        });

        Self {
            account_view,
            account_session_factory,
            mail_permission_requester,
            account_session: None,
            session_open_error: None,
            navigation_surface_appearance: NavigationSurfaceAppearance::Opaque,
            content: WorkspaceContent::Accounts,
            appearance_preference: AppearancePreference::System,
            appearance_settings_store: None,
            appearance_settings_notice: None,
            _appearance_subscription: None,
            _account_view_subscription: account_view_subscription,
            _account_view_observation: account_view_observation,
            mail_navigation_focus: cx.focus_handle().tab_stop(true),
            calendar_navigation_focus: cx.focus_handle().tab_stop(true),
            accounts_navigation_focus: cx.focus_handle().tab_stop(true),
            settings_navigation_focus: cx.focus_handle().tab_stop(true),
            task: None,
            #[cfg(feature = "ui-preview")]
            preview_start: None,
            #[cfg(feature = "ui-preview")]
            _preview_message_observation: None,
        }
    }
}

impl<P: AccountProvider + 'static> Workspace<P> {
    pub(crate) fn new_for_window(
        services: Result<AccountServices<P>, SetupIssue>,
        store: std::io::Result<AppearanceSettingsStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_appearance_store(services, store, window, cx)
    }

    fn new_with_appearance_store(
        services: Result<AccountServices<P>, SetupIssue>,
        store: std::io::Result<AppearanceSettingsStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::new(services, cx);
        workspace.load_appearance_settings(store);
        workspace.apply_appearance(window, cx);
        workspace._appearance_subscription = Some(cx.observe_window_appearance(
            window,
            |workspace, window, cx| {
                if workspace.appearance_preference == AppearancePreference::System {
                    workspace.apply_appearance(window, cx);
                }
            },
        ));
        workspace
    }

    #[cfg(test)]
    pub(crate) fn new_with_appearance_store_for_test(
        services: Result<AccountServices<P>, SetupIssue>,
        store: std::io::Result<AppearanceSettingsStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_appearance_store(services, store, window, cx)
    }

    fn load_appearance_settings(&mut self, store: std::io::Result<AppearanceSettingsStore>) {
        match store {
            Ok(store) => {
                self.appearance_preference = match store.load() {
                    Ok(appearance) => appearance,
                    Err(_) => {
                        self.appearance_settings_notice =
                            Some(AppearanceSettingsNotice::LoadFailed);
                        AppearancePreference::System
                    }
                };
                self.appearance_settings_store = Some(store);
            }
            Err(_) => {
                self.appearance_settings_notice = Some(AppearanceSettingsNotice::LoadFailed);
            }
        }
    }

    fn apply_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mode = self.appearance_preference.theme_mode(window.appearance());
        gpui_kit::component::Theme::change(mode, Some(window), cx);
    }

    pub(crate) fn with_navigation_surface_appearance(
        mut self,
        appearance: NavigationSurfaceAppearance,
    ) -> Self {
        self.navigation_surface_appearance = appearance;
        self
    }
}
