use std::sync::Arc;

use einfach_mail_application::{
    AccountCacheRemovalError, AccountConnectionRequest, AccountOperationCancellation,
    AccountOperationError, AccountProvider, AccountSessionFactory, AccountWorkflow,
    ConnectedAccount, ExchangeEwsConnectionInput, ExchangeEwsDiscoveryConfirmation,
    ExchangeEwsDiscoveryResult,
};
use einfach_mail_domain::AccountProfile;
use gpui_kit::component::input::InputState;
use gpui_kit::{AppContext, Context, Entity, Window};

use super::AccountView;

// Preview startup bypasses the production configuration errors represented here.
#[cfg_attr(feature = "ui-preview", allow(dead_code))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SetupIssue {
    MissingClientId,
    InvalidClientId,
    MissingCallbackPort,
    InvalidCallbackPort,
    ExchangeSettingsUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SavedCredentialState {
    Absent,
    MayExist,
}

impl SavedCredentialState {
    pub(super) fn after_connection_error(error: AccountOperationError) -> Self {
        match error {
            AccountOperationError::SavedAccountIdentityUnavailable
            | AccountOperationError::AccountMismatch => Self::MayExist,
            _ => Self::Absent,
        }
    }
}

pub(crate) struct AccountServices<P> {
    pub(super) workflow: Arc<AccountWorkflow<P>>,
    pub(super) account_session_factory: Arc<dyn AccountSessionFactory>,
    pub(super) microsoft365_setup_issue: Option<SetupIssue>,
}

impl<P> AccountServices<P> {
    pub(crate) fn new(
        workflow: Arc<AccountWorkflow<P>>,
        account_session_factory: Arc<dyn AccountSessionFactory>,
    ) -> Self {
        Self {
            workflow,
            account_session_factory,
            microsoft365_setup_issue: None,
        }
    }

    #[cfg(any(test, not(feature = "ui-preview")))]
    pub(crate) fn with_microsoft365_setup_issue(mut self, issue: SetupIssue) -> Self {
        self.microsoft365_setup_issue = Some(issue);
        self
    }
}

#[derive(Clone)]
pub(super) enum AccountStatus {
    Restoring,
    Disconnected,
    Connecting {
        cancellation: AccountOperationCancellation,
    },
    CancellingConnect,
    SigningOut {
        account: Option<ConnectedAccount>,
    },
    Connected(ConnectedAccount),
    Reauthorizing {
        account: ConnectedAccount,
        cancellation: AccountOperationCancellation,
    },
    CancellingReauthorization {
        account: ConnectedAccount,
    },
    ReauthorizationFailed {
        account: ConnectedAccount,
        error: AccountOperationError,
    },
    SignOutFailed {
        account: Option<ConnectedAccount>,
        error: AccountOperationError,
    },
    Failed {
        error: AccountOperationError,
        saved_credential_state: SavedCredentialState,
    },
}

impl AccountStatus {
    pub(super) fn saved_credential_state(&self) -> SavedCredentialState {
        match self {
            Self::Disconnected | Self::Connecting { .. } | Self::CancellingConnect => {
                SavedCredentialState::Absent
            }
            Self::Failed {
                saved_credential_state,
                ..
            } => *saved_credential_state,
            Self::SigningOut { .. }
            | Self::Connected(_)
            | Self::Reauthorizing { .. }
            | Self::CancellingReauthorization { .. }
            | Self::ReauthorizationFailed { .. }
            | Self::SignOutFailed { .. }
            | Self::Restoring => SavedCredentialState::MayExist,
        }
    }

    pub(super) fn connected_profile(&self) -> Option<&AccountProfile> {
        self.connected_account().map(ConnectedAccount::profile)
    }

    pub(super) fn connected_account(&self) -> Option<&ConnectedAccount> {
        match self {
            Self::Connected(account)
            | Self::Reauthorizing { account, .. }
            | Self::CancellingReauthorization { account }
            | Self::ReauthorizationFailed { account, .. } => Some(account),
            Self::SigningOut {
                account: Some(account),
            }
            | Self::SignOutFailed {
                account: Some(account),
                ..
            } => Some(account),
            _ => None,
        }
    }
}

pub(super) struct ExchangeEwsConnectionForm {
    pub(super) endpoint: Entity<InputState>,
    pub(super) mailbox_address: Entity<InputState>,
    pub(super) username: Entity<InputState>,
    pub(super) password: Entity<InputState>,
    pub(super) discovery: EwsDiscoveryState,
}

pub(super) enum EwsDiscoveryState {
    Idle,
    Searching,
    Confirmation(ExchangeEwsDiscoveryConfirmation),
    Cancelling,
    Found,
    Failed,
}

impl EwsDiscoveryState {
    pub(super) fn allows_start(&self) -> bool {
        matches!(self, Self::Idle | Self::Failed | Self::Found)
    }

    pub(super) fn is_busy(&self) -> bool {
        matches!(self, Self::Searching | Self::Cancelling)
    }

    pub(super) fn requires_confirmation(&self) -> bool {
        matches!(self, Self::Confirmation(_))
    }

    pub(super) fn from_result(
        discovery_result: Result<ExchangeEwsDiscoveryResult, AccountOperationError>,
    ) -> Self {
        match discovery_result {
            Ok(ExchangeEwsDiscoveryResult::ConfirmationRequired(confirmation)) => {
                Self::Confirmation(confirmation)
            }
            Ok(ExchangeEwsDiscoveryResult::Cancelled) => Self::Idle,
            _ => Self::Failed,
        }
    }
}

impl ExchangeEwsConnectionForm {
    pub(super) fn new<P: AccountProvider + 'static>(
        window: &mut Window,
        cx: &mut Context<AccountView<P>>,
    ) -> Self {
        Self {
            endpoint: cx.new(|cx| {
                InputState::new(window, cx).placeholder("https://mail.example/EWS/Exchange.asmx")
            }),
            mailbox_address: cx
                .new(|cx| InputState::new(window, cx).placeholder("name@example.com")),
            username: cx.new(|cx| InputState::new(window, cx).placeholder("DOMAIN\\username")),
            password: cx.new(|cx| InputState::new(window, cx).masked(true)),
            discovery: EwsDiscoveryState::Idle,
        }
    }

    pub(super) fn request<P: AccountProvider + 'static>(
        &self,
        cx: &Context<AccountView<P>>,
    ) -> AccountConnectionRequest {
        AccountConnectionRequest::ExchangeEws(ExchangeEwsConnectionInput::new(
            self.endpoint.read(cx).value().to_string(),
            self.mailbox_address.read(cx).value().to_string(),
            self.username.read(cx).value().to_string(),
            self.password.read(cx).value().to_string(),
        ))
    }
}

pub(super) enum AccountSignOutError {
    Identity(AccountOperationError),
    CacheRemoval(AccountCacheRemovalError),
    CredentialRemoval(AccountOperationError),
}
