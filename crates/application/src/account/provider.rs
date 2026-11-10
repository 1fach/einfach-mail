use std::{fmt, future::Future};

use einfach_mail_domain::{AccountId, AccountProfile};
use secrecy::{ExposeSecret, SecretString};
use thiserror::Error;

use super::workflow::AccountOperationCancellation;

/// A user-safe category for an account workflow failure.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AccountOperationError {
    /// The user cancelled browser authorization.
    #[error("sign-in was cancelled")]
    SignInCancelled,
    /// The authorization server did not grant the requested access.
    #[error("access was not granted")]
    AccessDenied,
    /// The connected account does not support the requested operation.
    #[error("this account does not support that operation")]
    UnsupportedOperation,
    /// The supplied Exchange endpoint or sign-in identity is invalid.
    #[error("Exchange server details are invalid")]
    InvalidConnectionDetails,
    /// Exchange Autodiscover could not find a usable EWS endpoint.
    #[error("Exchange Autodiscover did not find a usable EWS endpoint")]
    AutodiscoverFailed,
    /// Microsoft rejected the access token used for a Graph profile request.
    #[error("the access token was rejected")]
    AccessTokenRejected,
    /// A stored refresh token can no longer restore the account.
    #[error("saved sign-in has expired")]
    SignInExpired,
    /// The Linux credential service could not read, store, or remove a token.
    #[error("the Linux credential service is unavailable")]
    CredentialStoreUnavailable,
    /// The provider returned an incomplete account profile.
    #[error("the account profile is incomplete")]
    ProfileUnavailable,
    /// A network or browser operation failed.
    #[error("the account service is unavailable")]
    ServiceUnavailable,
    /// Sign-in failed for a reason that does not have a more specific category.
    #[error("sign-in failed")]
    SignInFailed,
    /// The single-account workflow already has an active account.
    #[error("an account is already connected")]
    AccountAlreadyConnected,
    /// No account is active for a permission update.
    #[error("no account is connected")]
    AccountNotConnected,
    /// The account selected during permission renewal does not match the active account.
    #[error("the selected account does not match the connected account")]
    AccountMismatch,
    /// A saved credential has no Graph-verified identity.
    #[error("the saved sign-in identity is not verified")]
    SavedAccountIdentityUnavailable,
    /// Another account operation is still running.
    #[error("another account operation is still running")]
    AccountOperationInProgress,
    /// The workflow could not read or update its local account state.
    #[error("account workflow state is unavailable")]
    AccountStateUnavailable,
}

/// Selects the provider and sign-in details for a new account.
pub enum AccountConnectionRequest {
    /// Connects a Microsoft 365 account through delegated Microsoft Graph access.
    Microsoft365,
    /// Connects an Exchange Server account through EWS and NTLMv2.
    ExchangeEws(ExchangeEwsConnectionInput),
}

impl fmt::Debug for AccountConnectionRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Microsoft365 => formatter.write_str("Microsoft365"),
            Self::ExchangeEws(_) => formatter.write_str("ExchangeEws(<redacted>)"),
        }
    }
}

/// User-supplied values for an Exchange Server EWS connection.
pub struct ExchangeEwsConnectionInput {
    endpoint: String,
    mailbox_address: String,
    username: String,
    password: SecretString,
}

/// A pending Autodiscover destination that needs approval before sending an address.
#[derive(Clone, Eq, PartialEq)]
pub struct ExchangeEwsDiscoveryConfirmation {
    challenge: u64,
    mailbox_address: String,
    destination: String,
}

impl ExchangeEwsDiscoveryConfirmation {
    /// Creates a confirmation request returned by an account provider.
    #[must_use]
    pub fn new(challenge: u64, mailbox_address: String, destination: String) -> Self {
        Self {
            challenge,
            mailbox_address,
            destination,
        }
    }

    /// Returns the mailbox address that would be sent after confirmation.
    #[must_use]
    pub fn mailbox_address(&self) -> &str {
        &self.mailbox_address
    }

    /// Returns the HTTPS URL that would receive the mailbox address.
    #[must_use]
    pub fn destination(&self) -> &str {
        &self.destination
    }

    /// Returns the provider-issued challenge identifier.
    #[must_use]
    pub fn challenge(&self) -> u64 {
        self.challenge
    }
}

impl fmt::Debug for ExchangeEwsDiscoveryConfirmation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExchangeEwsDiscoveryConfirmation")
            .field("challenge", &"<redacted>")
            .field("mailbox_address", &"<redacted>")
            .field("destination", &"<redacted>")
            .finish()
    }
}

/// Starts discovery or answers a pending Autodiscover confirmation.
#[non_exhaustive]
pub enum ExchangeEwsDiscoveryRequest {
    /// Starts unauthenticated endpoint discovery for the mailbox address.
    Start {
        /// Mailbox address used to derive Autodiscover destinations.
        mailbox_address: String,
    },
    /// Continues discovery after the user approved the displayed destination.
    Continue(ExchangeEwsDiscoveryConfirmation),
    /// Cancels a pending destination request without contacting its host.
    Cancel(ExchangeEwsDiscoveryConfirmation),
}

impl fmt::Debug for ExchangeEwsDiscoveryRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Start { .. } => formatter.write_str("Start(<redacted>)"),
            Self::Continue(_) => formatter.write_str("Continue(<redacted>)"),
            Self::Cancel(_) => formatter.write_str("Cancel(<redacted>)"),
        }
    }
}

/// The result of an Exchange EWS Autodiscover operation.
#[non_exhaustive]
pub enum ExchangeEwsDiscoveryResult {
    /// Returns the validated EWS HTTPS endpoint for the editable account form.
    Endpoint(String),
    /// Pauses before sending the mailbox address to a URL that needs approval.
    ConfirmationRequired(ExchangeEwsDiscoveryConfirmation),
    /// Reports that the user cancelled a pending destination request.
    Cancelled,
}

impl fmt::Debug for ExchangeEwsDiscoveryResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Endpoint(_) => formatter.write_str("Endpoint(<redacted>)"),
            Self::ConfirmationRequired(_) => formatter.write_str("ConfirmationRequired"),
            Self::Cancelled => formatter.write_str("Cancelled"),
        }
    }
}

impl ExchangeEwsConnectionInput {
    /// Creates EWS connection input; the EWS provider validates the endpoint and identity.
    #[must_use]
    pub fn new(
        endpoint: String,
        mailbox_address: String,
        username: String,
        password: String,
    ) -> Self {
        Self {
            endpoint,
            mailbox_address,
            username,
            password: SecretString::from(password),
        }
    }

    /// Returns the user-entered EWS endpoint for provider-side validation.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Returns the user-entered mailbox address for provider-side validation.
    #[must_use]
    pub fn mailbox_address(&self) -> &str {
        &self.mailbox_address
    }

    /// Returns the user-entered NTLM identity for provider-side validation.
    #[must_use]
    pub fn username(&self) -> &str {
        &self.username
    }

    /// Exposes the password to the EWS provider for authentication and storage.
    #[must_use]
    pub fn expose_password(&self) -> &str {
        self.password.expose_secret()
    }
}

impl fmt::Debug for ExchangeEwsConnectionInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExchangeEwsConnectionInput")
            .field("endpoint", &"<redacted>")
            .field("mailbox_address", &"<redacted>")
            .field("username", &"<redacted>")
            .field("password", &"<redacted>")
            .finish()
    }
}

/// The delegated permissions requested during same-account authorization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountPermissionRequest {
    /// Requests the permissions already used by the connected account.
    Current,
    /// Adds delegated permission to send mail as the signed-in account.
    SendMail,
    /// Adds permission to create, read, update, and delete mail for the user.
    MailReadWrite,
    /// Adds permission to create, read, update, and delete calendar events for the user.
    CalendarReadWrite,
}

/// Operations the application needs from a connected-account provider.
pub trait AccountProvider: Send + Sync {
    /// Restores the saved account, if the provider has a refresh credential.
    ///
    /// # Errors
    ///
    /// Returns a provider error when the credential cannot be loaded, renewed,
    /// or mapped to an account profile.
    fn restore_account(
        &self,
    ) -> impl Future<Output = Result<Option<AccountProfile>, AccountOperationError>> + Send + '_;

    /// Returns a verified account ID that can identify the cache to remove.
    ///
    /// `Ok(None)` means an unverified saved credential or the absence of both
    /// a saved credential and an active session prevents safe cache removal.
    ///
    /// # Errors
    ///
    /// Returns [`AccountOperationError::AccountMismatch`] when the saved
    /// credential and active session identify different accounts, or a
    /// provider error when the credential cannot be read.
    fn saved_credential_account_id(
        &self,
    ) -> impl Future<Output = Result<Option<AccountId>, AccountOperationError>> + Send + '_;

    /// Connects the selected account through its provider's sign-in flow.
    ///
    /// Implementations must call [`AccountOperationCancellation::begin_credential_commit`]
    /// immediately before storing the refresh credential.
    ///
    /// # Errors
    ///
    /// Returns [`AccountOperationError::SignInCancelled`] when cancellation
    /// wins before credential commit,
    /// [`AccountOperationError::SavedAccountIdentityUnavailable`] when an
    /// unverified saved credential blocks replacement sign-in, or
    /// [`AccountOperationError::AccountMismatch`] when the saved identity
    /// differs from the newly authorized account. Returns another provider
    /// error when authorization, profile loading, or credential storage fails.
    fn connect_account(
        &self,
        cancellation: AccountOperationCancellation,
    ) -> impl Future<Output = Result<AccountProfile, AccountOperationError>> + Send + '_;

    /// Connects the selected account through its provider's sign-in flow.
    ///
    /// Providers that do not support the selected connection request return
    /// [`AccountOperationError::UnsupportedOperation`].
    ///
    /// # Errors
    ///
    /// Returns [`AccountOperationError::UnsupportedOperation`] when the
    /// provider does not support this request, or another provider error when
    /// the connection cannot be established.
    fn connect_account_with_request(
        &self,
        request: AccountConnectionRequest,
        cancellation: AccountOperationCancellation,
    ) -> impl Future<Output = Result<AccountProfile, AccountOperationError>> + Send + '_ {
        async move {
            match request {
                AccountConnectionRequest::Microsoft365 => self.connect_account(cancellation).await,
                AccountConnectionRequest::ExchangeEws(_) => {
                    Err(AccountOperationError::UnsupportedOperation)
                }
            }
        }
    }

    /// Discovers an Exchange EWS endpoint without receiving account credentials.
    ///
    /// Implementations must not send a username, password, or authentication
    /// token during discovery. They return a confirmation request before
    /// sending the mailbox address to a destination that needs approval.
    ///
    /// # Errors
    ///
    /// Returns [`AccountOperationError::UnsupportedOperation`] when the
    /// provider does not support EWS discovery, or another provider error when
    /// the endpoint cannot be discovered.
    fn discover_exchange_ews_endpoint(
        &self,
        _request: ExchangeEwsDiscoveryRequest,
    ) -> impl Future<Output = Result<ExchangeEwsDiscoveryResult, AccountOperationError>> + Send + '_
    {
        async { Err(AccountOperationError::UnsupportedOperation) }
    }

    /// Requests the selected delegated permissions again for the active account.
    ///
    /// Implementations must preserve the existing credential and session when
    /// authorization is cancelled, denied, or returns another account.
    /// They must call [`AccountOperationCancellation::begin_credential_commit`]
    /// immediately before storing a replacement refresh credential.
    ///
    /// # Errors
    ///
    /// Returns [`AccountOperationError::SignInCancelled`] when cancellation
    /// wins before credential commit, [`AccountOperationError::AccountMismatch`]
    /// when authorization returns another account, or another provider error
    /// when authorization or credential storage fails.
    fn reauthorize_account(
        &self,
        expected_account_id: &AccountId,
        request: AccountPermissionRequest,
        cancellation: AccountOperationCancellation,
    ) -> impl Future<Output = Result<AccountProfile, AccountOperationError>> + Send + '_;

    /// Removes the provider's locally saved refresh credential.
    ///
    /// # Errors
    ///
    /// Returns [`AccountOperationError::SavedAccountIdentityUnavailable`]
    /// when the credential has no Graph-verified identity, or another provider
    /// error when the credential cannot be removed. Returns
    /// [`AccountOperationError::AccountMismatch`] when the saved identity
    /// differs from the active provider session.
    fn sign_out(&self) -> impl Future<Output = Result<(), AccountOperationError>> + Send + '_;
}
