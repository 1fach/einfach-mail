use std::sync::Arc;

use einfach_mail_application::{
    AccountCacheOpenError, AccountOperationError, AccountPermissionRequest, AccountProvider,
    AccountSession, ConnectedAccount,
};
use einfach_mail_domain::AccountId;
use gpui_kit::{App, Background, FocusHandle};

use super::{
    AccountContent, AccountStatus, AccountView, AccountViewCacheError, NavigationSurfaceAppearance,
    OpeningContent, SavedCredentialState, WorkspaceSection,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AccountViewTestContent {
    Account,
    Settings,
    OpeningMail,
    OpeningCalendar,
    Mail,
    Calendar,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AccountViewTestSection {
    Mail,
    Calendar,
    Accounts,
    Settings,
}

impl<P: AccountProvider + 'static> AccountView<P> {
    pub(crate) fn test_set_connected_account(&mut self, account: ConnectedAccount) {
        self.status = AccountStatus::Connected(account);
    }

    pub(crate) fn test_set_opening_mail_content(&mut self) {
        self.content = AccountContent::Opening(OpeningContent::Mail);
    }

    pub(crate) fn test_set_account_session(&mut self, session: Option<Arc<dyn AccountSession>>) {
        self.account_session = session;
    }

    pub(crate) fn test_content(&self) -> AccountViewTestContent {
        match self.content {
            AccountContent::Account => AccountViewTestContent::Account,
            AccountContent::Settings => AccountViewTestContent::Settings,
            AccountContent::Opening(OpeningContent::Mail) => AccountViewTestContent::OpeningMail,
            AccountContent::Opening(OpeningContent::Calendar) => {
                AccountViewTestContent::OpeningCalendar
            }
            AccountContent::Mail { .. } => AccountViewTestContent::Mail,
            AccountContent::Calendar { .. } => AccountViewTestContent::Calendar,
        }
    }

    pub(crate) fn test_selected_section(&self) -> AccountViewTestSection {
        match self.content.selected_section() {
            WorkspaceSection::Mail => AccountViewTestSection::Mail,
            WorkspaceSection::Calendar => AccountViewTestSection::Calendar,
            WorkspaceSection::Accounts => AccountViewTestSection::Accounts,
            WorkspaceSection::Settings => AccountViewTestSection::Settings,
        }
    }

    pub(crate) fn test_status_is_connected_to(&self, account_id: &AccountId) -> bool {
        matches!(
            &self.status,
            AccountStatus::Connected(account) if account.profile().id() == account_id
        )
    }

    pub(crate) fn test_status_is_disconnected(&self) -> bool {
        matches!(self.status, AccountStatus::Disconnected)
    }

    pub(crate) fn test_status_is_failed_with_possible_saved_credential(
        &self,
        expected_error: AccountOperationError,
    ) -> bool {
        matches!(
            &self.status,
            AccountStatus::Failed {
                error,
                saved_credential_state: SavedCredentialState::MayExist,
            } if error == &expected_error
        )
    }

    pub(crate) fn test_session_open_failed_with(
        &self,
        expected_error: AccountCacheOpenError,
    ) -> bool {
        matches!(
            &self.session_open_error,
            Some(AccountViewCacheError::Open(error)) if error == &expected_error
        )
    }

    pub(crate) fn test_session_open_error_is_none(&self) -> bool {
        self.session_open_error.is_none()
    }

    pub(crate) fn test_has_pending_permission_review(&self) -> bool {
        self.pending_permission_review.is_some()
    }

    pub(crate) fn test_pending_permission_review_is(
        &self,
        expected_request: Option<AccountPermissionRequest>,
    ) -> bool {
        self.pending_permission_review == expected_request
    }

    pub(crate) fn test_has_account_session(&self) -> bool {
        self.account_session.is_some()
    }

    pub(crate) fn test_session_matches(&self, expected_session: &Arc<dyn AccountSession>) -> bool {
        self.account_session
            .as_ref()
            .is_some_and(|session| Arc::ptr_eq(session, expected_session))
    }

    pub(crate) fn test_appearance_preference(&self) -> super::AppearancePreference {
        self.appearance_preference
    }

    pub(crate) fn test_appearance_settings_notice(
        &self,
    ) -> Option<super::AppearanceSettingsNotice> {
        self.appearance_settings_notice
    }

    pub(crate) fn test_navigation_disabled_reason(
        &self,
        section: AccountViewTestSection,
        cx: &App,
    ) -> Option<&'static str> {
        let section = match section {
            AccountViewTestSection::Mail => WorkspaceSection::Mail,
            AccountViewTestSection::Calendar => WorkspaceSection::Calendar,
            AccountViewTestSection::Accounts => WorkspaceSection::Accounts,
            AccountViewTestSection::Settings => WorkspaceSection::Settings,
        };
        self.navigation_disabled_reason(section, cx)
    }

    pub(crate) fn test_navigation_focus(&self, section: AccountViewTestSection) -> FocusHandle {
        match section {
            AccountViewTestSection::Mail => self.mail_navigation_focus.clone(),
            AccountViewTestSection::Calendar => self.calendar_navigation_focus.clone(),
            AccountViewTestSection::Accounts => self.accounts_navigation_focus.clone(),
            AccountViewTestSection::Settings => self.settings_navigation_focus.clone(),
        }
    }
}

impl NavigationSurfaceAppearance {
    pub(crate) fn test_navigation_background(self, sidebar: Background) -> Background {
        self.navigation_background(sidebar)
    }

    pub(crate) fn test_opacity() -> f32 {
        super::NAVIGATION_SURFACE_OPACITY
    }
}
