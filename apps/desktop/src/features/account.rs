use std::sync::Arc;

mod actions;
mod copy;
mod render;
mod state;

use einfach_mail_application::{
    AccountCacheOpenError, AccountCacheRemovalError, AccountPermissionRequest, AccountProvider,
    AccountSessionFactory, AccountWorkflow,
};
use einfach_mail_domain::{AccountId, AccountProfile, CalendarAction};
use gpui_kit::component::ActiveTheme;
use gpui_kit::{Context, EventEmitter, IntoElement, Render, Task, Window};

#[cfg(test)]
use copy::{
    account_cache_removal_error_message, calendar_actions_summary, permission_review_disclosure,
};
pub(crate) use state::{AccountServices, SetupIssue};
use state::{AccountStatus, ExchangeEwsConnectionForm};
#[cfg(test)]
use state::{EwsDiscoveryState, SavedCredentialState};

pub(crate) enum AccountViewEvent {
    RestoreStarted,
    SignOutRequested,
    SignOutFinished(SignOutSessionDisposition),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SignOutSessionDisposition {
    Retain,
    Release,
}

pub(crate) struct AccountView<P: AccountProvider + 'static> {
    services: Result<AccountServices<P>, SetupIssue>,
    status: AccountStatus,
    pending_permission_review: Option<AccountPermissionRequest>,
    cache_removal_error: Option<AccountCacheRemovalError>,
    exchange_ews_form: Option<ExchangeEwsConnectionForm>,
    workspace_is_opening: bool,
    task: Option<Task<()>>,
}

impl<P: AccountProvider + 'static> AccountView<P> {
    pub(crate) fn new(
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
            status,
            pending_permission_review: None,
            cache_removal_error: None,
            exchange_ews_form: None,
            workspace_is_opening: false,
            task: None,
        };
        view.start_restore(cx);
        view
    }

    pub(crate) fn connected_profile_for_workspace(&self) -> Option<AccountProfile> {
        match &self.status {
            AccountStatus::Connected(account) => Some(account.profile().clone()),
            _ => None,
        }
    }

    pub(crate) fn navigation_profile(&self) -> Option<AccountProfile> {
        self.status.connected_profile().cloned()
    }

    pub(crate) fn navigation_disabled_reason(&self) -> Option<&'static str> {
        if let Err(issue) = &self.services {
            return Some(issue.message());
        }
        match &self.status {
            AccountStatus::Connected(_) => None,
            AccountStatus::Disconnected => Some("Connect an account to use this section."),
            _ => Some("Resolve the account status in Accounts before using this section."),
        }
    }

    pub(crate) fn calendar_is_available(&self) -> bool {
        self.status.connected_profile().is_some_and(|profile| {
            profile
                .capabilities()
                .calendar()
                .supports(CalendarAction::ViewEvents)
        })
    }

    pub(crate) fn is_connected_to(&self, account_id: &AccountId) -> bool {
        matches!(
            &self.status,
            AccountStatus::Connected(account) if account.profile().id() == account_id
        )
    }

    pub(crate) fn set_workspace_opening(&mut self, opening: bool, cx: &mut Context<Self>) {
        if self.workspace_is_opening == opening {
            return;
        }
        self.workspace_is_opening = opening;
        cx.notify();
    }

    pub(crate) fn clear_cache_removal_error(&mut self, cx: &mut Context<Self>) {
        if self.cache_removal_error.is_none() {
            return;
        }
        self.cache_removal_error = None;
        cx.notify();
    }

    pub(crate) fn session_factory(&self) -> Option<Arc<dyn AccountSessionFactory>> {
        self.services
            .as_ref()
            .ok()
            .map(|services| Arc::clone(&services.account_session_factory))
    }

    pub(crate) fn workflow(&self) -> Option<Arc<AccountWorkflow<P>>> {
        self.services
            .as_ref()
            .ok()
            .map(|services| Arc::clone(&services.workflow))
    }
}

impl<P: AccountProvider + 'static> EventEmitter<AccountViewEvent> for AccountView<P> {}

impl<P: AccountProvider + 'static> Render for AccountView<P> {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_account_feature_content(cx.theme().semantic_tokens().colors, cx)
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod test_access;

pub(crate) fn account_cache_open_error_message(error: AccountCacheOpenError) -> &'static str {
    copy::account_cache_open_error_message(error)
}

#[cfg(test)]
pub(crate) fn test_account_cache_open_error_message(error: AccountCacheOpenError) -> &'static str {
    account_cache_open_error_message(error)
}
