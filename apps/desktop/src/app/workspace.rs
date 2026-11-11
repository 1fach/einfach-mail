use std::sync::Arc;

#[path = "../features/account/actions.rs"]
mod account_actions;
#[path = "../features/account/copy.rs"]
mod account_copy;
#[path = "../features/account/render.rs"]
mod account_render;
#[path = "../features/account/state.rs"]
mod account_state;

use crate::features::{
    AppearancePreference, AppearanceSettingsNotice, AppearanceSettingsStore, CalendarView,
    CalendarViewEvent, MailFolderNavigation, MailPermissionRequester, MailView, MailViewEvent,
    SettingsViewState, render_settings,
};
#[cfg(test)]
use account_copy::{
    account_cache_open_error_message, account_cache_removal_error_message,
    permission_review_disclosure,
};
#[cfg(test)]
use account_state::EwsDiscoveryState;
pub(super) use account_state::{AccountServices, SetupIssue};
use account_state::{
    AccountSignOutError, AccountStatus, AccountViewCacheError, ExchangeEwsConnectionForm,
};
use einfach_mail_application::{
    AccountCacheRemovalError, AccountOperationError, AccountPermissionRequest, AccountProvider,
    AccountSession, AccountSessionFactory, AccountWorkflow,
};
use einfach_mail_domain::{AccountProfile, CalendarAccess};
use gpui_kit::component::{
    ActiveTheme, ColorTokens, Disableable, Icon, IconName, Selectable, StyledExt,
    button::{Button, ButtonVariants},
};
#[cfg(test)]
use gpui_kit::test::TestSupportExt as _;
use gpui_kit::{
    App, AsyncApp, Background, Context, Entity, FocusHandle, IntoElement, Render, Role,
    Subscription, Task, Window, WindowBackgroundAppearance, div, prelude::*, rems,
};

pub(super) struct AccountView<P: AccountProvider + 'static> {
    services: Result<AccountServices<P>, SetupIssue>,
    navigation_surface_appearance: NavigationSurfaceAppearance,
    account_session: Option<Arc<dyn AccountSession>>,
    status: AccountStatus,
    pending_permission_review: Option<AccountPermissionRequest>,
    content: AccountContent,
    appearance_preference: AppearancePreference,
    appearance_settings_store: Option<AppearanceSettingsStore>,
    appearance_settings_notice: Option<AppearanceSettingsNotice>,
    _appearance_subscription: Option<Subscription>,
    mail_navigation_focus: FocusHandle,
    calendar_navigation_focus: FocusHandle,
    accounts_navigation_focus: FocusHandle,
    settings_navigation_focus: FocusHandle,
    session_open_error: Option<AccountViewCacheError>,
    exchange_ews_form: Option<ExchangeEwsConnectionForm>,
    task: Option<Task<()>>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum OpeningContent {
    Mail,
    Calendar,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum WorkspaceSection {
    Mail,
    Calendar,
    Accounts,
    Settings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NavigationSurfaceAppearance {
    Opaque,
    Translucent,
}

impl NavigationSurfaceAppearance {
    fn navigation_background(self, sidebar: Background) -> Background {
        match self {
            Self::Opaque => sidebar,
            Self::Translucent => sidebar.opacity(NAVIGATION_SURFACE_OPACITY),
        }
    }

    pub(super) fn window_background(self) -> WindowBackgroundAppearance {
        match self {
            Self::Opaque => WindowBackgroundAppearance::Opaque,
            Self::Translucent => WindowBackgroundAppearance::Transparent,
        }
    }
}

/// The navigation surface stays mostly opaque while letting a restrained desktop tint through.
const NAVIGATION_SURFACE_OPACITY: f32 = 0.88;

impl WorkspaceSection {
    fn navigation_identity(self) -> (&'static str, &'static str, IconName) {
        match self {
            Self::Mail => ("nav-mail", "Mail", IconName::Inbox),
            Self::Calendar => ("nav-calendar", "Calendar", IconName::Calendar),
            Self::Accounts => ("nav-accounts", "Accounts", IconName::User),
            Self::Settings => ("nav-settings", "Settings", IconName::Settings),
        }
    }
}

enum AccountContent {
    Account,
    Settings,
    Opening(OpeningContent),
    Mail {
        view: Entity<MailView>,
        folder_navigation: Entity<MailFolderNavigation>,
        subscription: Subscription,
    },
    Calendar {
        view: Entity<CalendarView>,
        subscription: Subscription,
    },
}

impl AccountContent {
    fn is_account(&self) -> bool {
        matches!(self, Self::Account)
    }

    fn opening_kind(&self) -> Option<OpeningContent> {
        match self {
            Self::Opening(opening) => Some(*opening),
            Self::Account | Self::Settings | Self::Mail { .. } | Self::Calendar { .. } => None,
        }
    }

    fn selected_section(&self) -> WorkspaceSection {
        match self {
            Self::Mail { .. } => WorkspaceSection::Mail,
            Self::Calendar { .. } => WorkspaceSection::Calendar,
            Self::Settings => WorkspaceSection::Settings,
            Self::Opening(OpeningContent::Mail) => WorkspaceSection::Mail,
            Self::Opening(OpeningContent::Calendar) => WorkspaceSection::Calendar,
            Self::Account => WorkspaceSection::Accounts,
        }
    }
}

impl<P: AccountProvider + 'static> AccountView<P> {
    pub(super) fn new(
        services: Result<AccountServices<P>, SetupIssue>,
        cx: &mut Context<Self>,
    ) -> Self {
        let status = if services.is_ok() {
            AccountStatus::Restoring
        } else {
            AccountStatus::Disconnected
        };
        let mut view = Self {
            services,
            navigation_surface_appearance: NavigationSurfaceAppearance::Opaque,
            account_session: None,
            status,
            pending_permission_review: None,
            content: AccountContent::Account,
            appearance_preference: AppearancePreference::System,
            appearance_settings_store: None,
            appearance_settings_notice: None,
            _appearance_subscription: None,
            mail_navigation_focus: cx.focus_handle().tab_stop(true),
            calendar_navigation_focus: cx.focus_handle().tab_stop(true),
            accounts_navigation_focus: cx.focus_handle().tab_stop(true),
            settings_navigation_focus: cx.focus_handle().tab_stop(true),
            session_open_error: None,
            exchange_ews_form: None,
            task: None,
        };
        view.start_restore(cx);
        view
    }

    pub(super) fn new_for_window(
        services: Result<AccountServices<P>, SetupIssue>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_appearance_store(services, AppearanceSettingsStore::new(), window, cx)
    }

    fn new_with_appearance_store(
        services: Result<AccountServices<P>, SetupIssue>,
        store: std::io::Result<AppearanceSettingsStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::new(services, cx);
        view.load_appearance_settings(store);
        view.apply_appearance(window, cx);
        view._appearance_subscription =
            Some(cx.observe_window_appearance(window, |view, window, cx| {
                if view.appearance_preference == AppearancePreference::System {
                    view.apply_appearance(window, cx);
                }
            }));
        view
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

    fn change_appearance(
        &mut self,
        appearance: AppearancePreference,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.appearance_preference == appearance && self.appearance_settings_notice.is_none() {
            return;
        }

        self.appearance_preference = appearance;
        self.appearance_settings_notice = match &self.appearance_settings_store {
            Some(store) if store.save(appearance).is_ok() => None,
            Some(_) | None => Some(AppearanceSettingsNotice::SaveFailed),
        };
        self.apply_appearance(window, cx);
        cx.notify();
    }

    pub(super) fn with_navigation_surface_appearance(
        mut self,
        appearance: NavigationSurfaceAppearance,
    ) -> Self {
        self.navigation_surface_appearance = appearance;
        self
    }

    fn sign_out(&mut self, cx: &mut Context<Self>) {
        let Ok(services) = &self.services else {
            return;
        };
        let workflow = Arc::clone(&services.workflow);
        if !self.status.saved_credential_may_exist()
            || self.content.opening_kind().is_some()
            || matches!(
                &self.status,
                AccountStatus::Connecting { .. }
                    | AccountStatus::CancellingConnect { .. }
                    | AccountStatus::Reauthorizing { .. }
                    | AccountStatus::CancellingReauthorization { .. }
                    | AccountStatus::SigningOut { .. }
            )
        {
            return;
        }

        let previous_status = self.status.clone();
        let account = previous_status.connected_account().cloned();
        let account_session_factory = Arc::clone(&services.account_session_factory);
        let account_session = self
            .close_content_view(cx)
            .or_else(|| self.account_session.clone());
        self.content = AccountContent::Account;
        self.session_open_error = None;
        self.status = AccountStatus::SigningOut { account };
        self.task = Some(Self::sign_out_task(
            workflow,
            previous_status,
            account_session,
            account_session_factory,
            cx,
        ));
        cx.notify();
    }

    fn close_content_view(&mut self, cx: &mut Context<Self>) -> Option<Arc<dyn AccountSession>> {
        match std::mem::replace(&mut self.content, AccountContent::Account) {
            AccountContent::Mail {
                view,
                folder_navigation,
                subscription,
            } => {
                drop(subscription);
                drop(folder_navigation);
                view.update(cx, |view, _| view.stop_pending_work());
            }
            AccountContent::Calendar { view, subscription } => {
                drop(subscription);
                view.update(cx, |view, _| view.stop_pending_work());
            }
            content => {
                self.content = content;
                return None;
            }
        }
        self.account_session.clone()
    }

    fn open_mail(&mut self, cx: &mut Context<Self>) {
        let Ok(services) = &self.services else {
            return;
        };
        let permission_requester: Arc<dyn MailPermissionRequester> = services.workflow.clone();
        self.open_account_content(
            OpeningContent::Mail,
            move |profile, session, cx| {
                let mail_view_session = session.mail_session();
                let mail_view = cx
                    .new(|cx| MailView::new(profile, mail_view_session, permission_requester, cx));
                let folder_navigation =
                    cx.new(|cx| MailFolderNavigation::new(mail_view.clone(), cx));
                let subscription = cx.subscribe(
                    &mail_view,
                    |view, _, event: &MailViewEvent, cx| match event {
                        MailViewEvent::ReviewMailWritePermission => {
                            drop(view.close_content_view(cx));
                            view.prepare_permission_review(
                                AccountPermissionRequest::MailReadWrite,
                                cx,
                            );
                        }
                    },
                );
                AccountContent::Mail {
                    view: mail_view,
                    folder_navigation,
                    subscription,
                }
            },
            cx,
        );
    }

    fn open_calendar(&mut self, cx: &mut Context<Self>) {
        if !self.calendar_is_available() {
            return;
        }
        self.open_account_content(
            OpeningContent::Calendar,
            |profile, session, cx| {
                let calendar_session = session.calendar_session();
                let calendar_view = cx.new(|cx| CalendarView::new(profile, calendar_session, cx));
                let subscription =
                    cx.subscribe(&calendar_view, |view, _, event: &CalendarViewEvent, cx| {
                        match event {
                            CalendarViewEvent::ReviewCalendarWritePermission => {
                                drop(view.close_content_view(cx));
                                view.prepare_permission_review(
                                    AccountPermissionRequest::CalendarReadWrite,
                                    cx,
                                );
                            }
                        }
                    });
                AccountContent::Calendar {
                    view: calendar_view,
                    subscription,
                }
            },
            cx,
        );
    }

    fn navigate_to(&mut self, section: WorkspaceSection, cx: &mut Context<Self>) {
        if self.content.selected_section() == section
            || self.navigation_disabled_reason(section, cx).is_some()
        {
            return;
        }

        drop(self.close_content_view(cx));
        self.content = AccountContent::Account;
        match section {
            WorkspaceSection::Accounts => cx.notify(),
            WorkspaceSection::Settings => {
                self.content = AccountContent::Settings;
                cx.notify();
            }
            WorkspaceSection::Mail => self.open_mail(cx),
            WorkspaceSection::Calendar => self.open_calendar(cx),
        }
    }

    fn navigation_disabled_reason(
        &self,
        section: WorkspaceSection,
        cx: &App,
    ) -> Option<&'static str> {
        if self.content.opening_kind().is_some() {
            return Some("Wait for the current section to finish opening.");
        }
        if !self.can_navigate_away(cx) {
            return Some("Wait for the current mail update to finish before switching sections.");
        }
        self.section_disabled_reason(section)
    }

    fn section_disabled_reason(&self, section: WorkspaceSection) -> Option<&'static str> {
        if matches!(
            section,
            WorkspaceSection::Accounts | WorkspaceSection::Settings
        ) {
            return None;
        }
        if let Err(issue) = &self.services {
            return Some(issue.message());
        }
        if !matches!(&self.status, AccountStatus::Connected(_)) {
            return Some(if matches!(&self.status, AccountStatus::Disconnected) {
                "Connect an account to use this section."
            } else {
                "Resolve the account status in Accounts before using this section."
            });
        }
        if section == WorkspaceSection::Calendar && !self.calendar_is_available() {
            return Some("Calendar is unavailable for this account.");
        }
        None
    }

    fn calendar_is_available(&self) -> bool {
        self.status
            .connected_profile()
            .is_some_and(|profile| profile.capabilities().calendar() != CalendarAccess::Unavailable)
    }

    fn navigation_focus(&self, section: WorkspaceSection) -> &FocusHandle {
        match section {
            WorkspaceSection::Mail => &self.mail_navigation_focus,
            WorkspaceSection::Calendar => &self.calendar_navigation_focus,
            WorkspaceSection::Accounts => &self.accounts_navigation_focus,
            WorkspaceSection::Settings => &self.settings_navigation_focus,
        }
    }

    fn can_navigate_away(&self, cx: &App) -> bool {
        match &self.content {
            AccountContent::Mail { view, .. } => view.read(cx).can_navigate_away(),
            AccountContent::Account
            | AccountContent::Settings
            | AccountContent::Opening(_)
            | AccountContent::Calendar { .. } => true,
        }
    }

    fn open_account_content(
        &mut self,
        opening: OpeningContent,
        build_content: impl FnOnce(
            AccountProfile,
            Arc<dyn AccountSession>,
            &mut Context<Self>,
        ) -> AccountContent
        + Send
        + 'static,
        cx: &mut Context<Self>,
    ) {
        if !self.content.is_account() {
            return;
        }
        let AccountStatus::Connected(account) = &self.status else {
            return;
        };
        let Ok(services) = &self.services else {
            return;
        };
        let account_session_factory = Arc::clone(&services.account_session_factory);
        let profile = account.profile().clone();
        let retained_session = self.account_session.clone();
        self.content = AccountContent::Opening(opening);
        self.session_open_error = None;
        self.task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            let session = match retained_session {
                Some(session) => Ok(session),
                None => {
                    let account_id = profile.id().clone();
                    cx.background_executor()
                        .spawn(
                            async move { account_session_factory.open_session(account_id).await },
                        )
                        .await
                }
            };
            let _ = this.update(cx, move |view, cx| {
                if view.content.opening_kind() != Some(opening)
                    || !matches!(
                        &view.status,
                        AccountStatus::Connected(current_profile)
                            if current_profile.profile().id() == profile.id()
                    )
                {
                    return;
                }
                view.task = None;
                view.content = AccountContent::Account;
                match session {
                    Ok(session) => {
                        view.content = build_content(profile, Arc::clone(&session), cx);
                        view.account_session = Some(session);
                    }
                    Err(error) => {
                        view.session_open_error = Some(AccountViewCacheError::Open(error));
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn sign_out_task(
        workflow: Arc<AccountWorkflow<P>>,
        previous_status: AccountStatus,
        account_session: Option<Arc<dyn AccountSession>>,
        account_session_factory: Arc<dyn AccountSessionFactory>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let result = cx
                .background_executor()
                .spawn(remove_verified_account_cache_and_sign_out(
                    workflow,
                    account_session,
                    account_session_factory,
                ))
                .await;
            let _ = this.update(cx, |view, cx| {
                view.finish_sign_out(previous_status, result, cx);
            });
        })
    }

    fn finish_sign_out(
        &mut self,
        previous_status: AccountStatus,
        result: Result<(), AccountSignOutError>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(()) => {
                self.account_session = None;
                self.status = AccountStatus::Disconnected;
            }
            Err(AccountSignOutError::Identity(error)) => {
                self.status = AccountStatus::Failed {
                    error,
                    saved_credential_may_exist: true,
                };
            }
            Err(AccountSignOutError::CacheRemoval(error)) => {
                self.status = previous_status;
                self.session_open_error = Some(AccountViewCacheError::Removal(error));
                if !matches!(
                    error,
                    AccountCacheRemovalError::OperationInProgress
                        | AccountCacheRemovalError::PendingMailSend
                        | AccountCacheRemovalError::PendingCalendarEventCreation
                ) {
                    self.account_session = None;
                }
            }
            Err(AccountSignOutError::CredentialRemoval(error)) => {
                self.account_session = None;
                self.status = AccountStatus::SignOutFailed {
                    account: previous_status.connected_account().cloned(),
                    error,
                };
            }
        }
        self.task = None;
        cx.notify();
    }

    fn render_navigation(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let navigation = div()
            .id("workspace-navigation")
            .role(Role::Navigation)
            .aria_label("Main navigation")
            .w(rems(16.))
            .min_w(rems(14.))
            .max_w(rems(20.))
            .h_full()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .bg(self
                .navigation_surface_appearance
                .navigation_background(cx.theme().tokens.sidebar.into()))
            .text_color(cx.theme().sidebar_foreground)
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .child(self.render_navigation_brand())
            .child(self.render_feature_navigation(cx));
        #[cfg(test)]
        let navigation = navigation.test_support();
        let mut navigation = navigation;
        if let Some(profile) = self.status.connected_profile() {
            navigation = navigation.child(Self::render_active_account_identity(profile, cx));
        }
        if let AccountContent::Mail {
            folder_navigation, ..
        } = &self.content
        {
            navigation = navigation.child(folder_navigation.clone());
        } else {
            navigation = navigation.child(div().flex_1());
        }
        navigation
            .child(self.render_account_navigation(cx))
            .child(self.render_settings_navigation(cx))
    }

    fn render_navigation_brand(&self) -> impl IntoElement {
        div()
            .px_4()
            .py_4()
            .child(div().text_lg().font_semibold().child("Einfach Mail"))
    }

    fn render_feature_navigation(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("workspace-feature-navigation")
            .flex()
            .flex_col()
            .gap_1()
            .px_2()
            .child(self.render_navigation_button(WorkspaceSection::Mail, cx))
            .child(self.render_navigation_button(WorkspaceSection::Calendar, cx))
    }

    fn render_active_account_identity(
        profile: &AccountProfile,
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
            .gap_2()
            .px_3()
            .py_3()
            .child(Icon::new(IconName::User).size_4())
            .child(Self::render_active_account_details(profile, colors));
        #[cfg(test)]
        let identity = identity.test_support();
        identity
    }

    fn render_active_account_details(
        profile: &AccountProfile,
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
            .gap_1()
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

    fn render_account_navigation(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("workspace-account-navigation")
            .border_t_1()
            .border_color(cx.theme().sidebar_border)
            .p_2()
            .child(self.render_navigation_button(WorkspaceSection::Accounts, cx))
    }

    fn render_settings_navigation(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("workspace-settings-navigation")
            .border_t_1()
            .border_color(cx.theme().sidebar_border)
            .p_2()
            .child(self.render_navigation_button(WorkspaceSection::Settings, cx))
    }

    fn render_navigation_button(
        &self,
        section: WorkspaceSection,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (id, label, icon) = section.navigation_identity();
        let selected = self.content.selected_section() == section;
        let disabled_reason = self.navigation_disabled_reason(section, cx);
        Button::new(id)
            .debug_selector(move || id.to_owned())
            .ghost()
            .selected(selected)
            .disabled(disabled_reason.is_some())
            .label(label)
            .icon(Icon::new(icon).size_4())
            .w_full()
            .track_focus(self.navigation_focus(section))
            .when_some(disabled_reason, |button, reason| button.tooltip(reason))
            .on_click(cx.listener(move |view, _, _, cx| view.navigate_to(section, cx)))
    }

    fn render_workspace_content(
        &self,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        match &self.content {
            AccountContent::Mail { view, .. } => {
                div().size_full().child(view.clone()).into_any_element()
            }
            AccountContent::Calendar { view, .. } => {
                div().size_full().child(view.clone()).into_any_element()
            }
            AccountContent::Settings => self.render_settings_workspace_content(colors, cx),
            AccountContent::Account | AccountContent::Opening(_) => {
                self.render_account_workspace_content(colors, cx)
            }
        }
    }

    fn render_settings_workspace_content(
        &self,
        colors: ColorTokens,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let on_change = cx.listener(|view, index, window, cx| {
            if let Some(appearance) = AppearancePreference::from_radio_index(*index) {
                view.change_appearance(appearance, window, cx);
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

impl<P: AccountProvider + 'static> Render for AccountView<P> {
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

async fn remove_account_cache(
    account_session: Option<Arc<dyn AccountSession>>,
    account_session_factory: Arc<dyn AccountSessionFactory>,
    account_id: einfach_mail_domain::AccountId,
) -> Result<(), AccountCacheRemovalError> {
    match account_session {
        Some(account_session) => account_session.remove_account_cache().await,
        None => {
            account_session_factory
                .remove_account_cache(&account_id)
                .await
        }
    }
}

async fn remove_verified_account_cache_and_sign_out<P: AccountProvider>(
    workflow: Arc<AccountWorkflow<P>>,
    account_session: Option<Arc<dyn AccountSession>>,
    account_session_factory: Arc<dyn AccountSessionFactory>,
) -> Result<(), AccountSignOutError> {
    let account_id = workflow
        .saved_credential_account_id()
        .await
        .map_err(AccountSignOutError::Identity)?
        .ok_or(AccountSignOutError::Identity(
            AccountOperationError::SavedAccountIdentityUnavailable,
        ))?;
    remove_account_cache(account_session, account_session_factory, account_id)
        .await
        .map_err(AccountSignOutError::CacheRemoval)?;
    workflow
        .sign_out()
        .await
        .map_err(AccountSignOutError::CredentialRemoval)
}

#[cfg(test)]
mod tests;
