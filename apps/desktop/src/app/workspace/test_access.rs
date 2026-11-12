use std::sync::Arc;

use einfach_mail_application::{
    AccountCacheOpenError, AccountOperationError, AccountPermissionRequest, AccountProvider,
    AccountSession, ConnectedAccount,
};
use einfach_mail_domain::AccountId;
use gpui_kit::{App, Background, FocusHandle};

use crate::features::{AppearancePreference, AppearanceSettingsNotice};

use super::state::{OpeningContent, WorkspaceContent, WorkspaceSection};
use super::{NavigationSurfaceAppearance, Workspace};
use crate::features::{CalendarView, MailView};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WorkspaceTestContent {
    Accounts,
    Settings,
    OpeningMail,
    OpeningCalendar,
    Mail,
    Calendar,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WorkspaceTestSection {
    Mail,
    Calendar,
    Accounts,
    Settings,
}

impl<P: AccountProvider + 'static> Workspace<P> {
    pub(crate) fn test_account_view(&self) -> gpui_kit::Entity<super::AccountView<P>> {
        self.account_view.clone()
    }

    pub(crate) fn test_mail_view(&self) -> Option<gpui_kit::Entity<MailView>> {
        match &self.content {
            WorkspaceContent::Mail { view, .. } => Some(view.clone()),
            WorkspaceContent::Accounts
            | WorkspaceContent::Settings
            | WorkspaceContent::Opening(_)
            | WorkspaceContent::Calendar { .. } => None,
        }
    }

    pub(crate) fn test_calendar_view(&self) -> Option<gpui_kit::Entity<CalendarView>> {
        match &self.content {
            WorkspaceContent::Calendar { view, .. } => Some(view.clone()),
            WorkspaceContent::Accounts
            | WorkspaceContent::Settings
            | WorkspaceContent::Opening(_)
            | WorkspaceContent::Mail { .. } => None,
        }
    }

    pub(crate) fn test_set_connected_account(
        &mut self,
        account: ConnectedAccount,
        cx: &mut gpui_kit::Context<Self>,
    ) {
        self.account_view.update(cx, |view, cx| {
            view.test_set_connected_account(account);
            cx.notify();
        });
    }

    pub(crate) fn test_set_opening_mail_content(&mut self, cx: &mut gpui_kit::Context<Self>) {
        self.content = WorkspaceContent::Opening(OpeningContent::Mail);
        self.account_view
            .update(cx, |view, cx| view.set_workspace_opening(true, cx));
    }

    pub(crate) fn test_set_account_session(&mut self, session: Option<Arc<dyn AccountSession>>) {
        self.account_session = session;
    }

    pub(crate) fn test_content(&self) -> WorkspaceTestContent {
        match self.content {
            WorkspaceContent::Accounts => WorkspaceTestContent::Accounts,
            WorkspaceContent::Settings => WorkspaceTestContent::Settings,
            WorkspaceContent::Opening(OpeningContent::Mail) => WorkspaceTestContent::OpeningMail,
            WorkspaceContent::Opening(OpeningContent::Calendar) => {
                WorkspaceTestContent::OpeningCalendar
            }
            WorkspaceContent::Mail { .. } => WorkspaceTestContent::Mail,
            WorkspaceContent::Calendar { .. } => WorkspaceTestContent::Calendar,
        }
    }

    pub(crate) fn test_selected_section(&self) -> WorkspaceTestSection {
        match self.content.selected_section() {
            WorkspaceSection::Mail => WorkspaceTestSection::Mail,
            WorkspaceSection::Calendar => WorkspaceTestSection::Calendar,
            WorkspaceSection::Accounts => WorkspaceTestSection::Accounts,
            WorkspaceSection::Settings => WorkspaceTestSection::Settings,
        }
    }

    pub(crate) fn test_status_is_connected_to(&self, account_id: &AccountId, cx: &App) -> bool {
        self.account_view
            .read(cx)
            .test_status_is_connected_to(account_id)
    }

    pub(crate) fn test_status_is_disconnected(&self, cx: &App) -> bool {
        self.account_view.read(cx).test_status_is_disconnected()
    }

    pub(crate) fn test_status_is_failed_with_possible_saved_credential(
        &self,
        expected_error: AccountOperationError,
        cx: &App,
    ) -> bool {
        self.account_view
            .read(cx)
            .test_status_is_failed_with_possible_saved_credential(expected_error)
    }

    pub(crate) fn test_session_open_failed_with(
        &self,
        expected_error: AccountCacheOpenError,
    ) -> bool {
        self.session_open_error == Some(expected_error)
    }

    pub(crate) fn test_session_open_error_is_none(&self) -> bool {
        self.session_open_error.is_none()
    }

    pub(crate) fn test_has_pending_permission_review(&self, cx: &App) -> bool {
        self.account_view
            .read(cx)
            .test_has_pending_permission_review()
    }

    pub(crate) fn test_pending_permission_review_is(
        &self,
        expected_request: Option<AccountPermissionRequest>,
        cx: &App,
    ) -> bool {
        self.account_view
            .read(cx)
            .test_pending_permission_review_is(expected_request)
    }

    pub(crate) fn test_has_account_session(&self) -> bool {
        self.account_session.is_some()
    }

    pub(crate) fn test_session_matches(&self, expected_session: &Arc<dyn AccountSession>) -> bool {
        self.account_session
            .as_ref()
            .is_some_and(|session| Arc::ptr_eq(session, expected_session))
    }

    pub(crate) fn test_appearance_preference(&self) -> AppearancePreference {
        self.appearance_preference
    }

    pub(crate) fn test_appearance_settings_notice(&self) -> Option<AppearanceSettingsNotice> {
        self.appearance_settings_notice
    }

    pub(crate) fn test_navigation_disabled_reason(
        &self,
        section: WorkspaceTestSection,
        cx: &App,
    ) -> Option<&'static str> {
        self.navigation_disabled_reason(section.into(), cx)
    }

    pub(crate) fn test_navigation_focus(&self, section: WorkspaceTestSection) -> FocusHandle {
        self.navigation_focus(section.into()).clone()
    }
}

impl From<WorkspaceTestSection> for WorkspaceSection {
    fn from(section: WorkspaceTestSection) -> Self {
        match section {
            WorkspaceTestSection::Mail => Self::Mail,
            WorkspaceTestSection::Calendar => Self::Calendar,
            WorkspaceTestSection::Accounts => Self::Accounts,
            WorkspaceTestSection::Settings => Self::Settings,
        }
    }
}

impl NavigationSurfaceAppearance {
    pub(crate) fn test_navigation_background(self, sidebar: Background) -> Background {
        self.navigation_background(sidebar)
    }

    pub(crate) fn test_opacity() -> f32 {
        super::state::NAVIGATION_SURFACE_OPACITY
    }
}
