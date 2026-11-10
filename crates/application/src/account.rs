//! Account connection workflows and their provider port.

use std::{
    fmt,
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
};

use einfach_mail_domain::{AccountId, AccountProfile};
use secrecy::{ExposeSecret, SecretString};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

/// Identifies the provider that established a connected account.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountProviderKind {
    /// Microsoft 365 through Microsoft Graph.
    MicrosoftGraph,
    /// Exchange Server through Exchange Web Services.
    ExchangeEws,
}

/// Pairs a provider-independent profile with the provider that established it.
///
/// Workflows return this value after restore, connection, and permission review
/// so consumers receive provider metadata with the corresponding identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectedAccount {
    profile: AccountProfile,
    provider_kind: AccountProviderKind,
}

impl ConnectedAccount {
    /// Creates an account result for a provider workflow.
    #[must_use]
    pub const fn new(profile: AccountProfile, provider_kind: AccountProviderKind) -> Self {
        Self {
            profile,
            provider_kind,
        }
    }

    /// Returns the provider-independent account identity and capabilities.
    #[must_use]
    pub const fn profile(&self) -> &AccountProfile {
        &self.profile
    }

    /// Returns the provider selected for this account.
    #[must_use]
    pub const fn provider_kind(&self) -> AccountProviderKind {
        self.provider_kind
    }
}

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

/// Signals cancellation for one account connection or permission renewal.
///
/// A request cancels asynchronous authorization until a provider starts the
/// credential commit. After that transition, the operation completes and
/// returns its actual result.
#[derive(Clone, Debug)]
pub struct AccountOperationCancellation {
    state: Arc<AccountOperationCancellationState>,
}

impl AccountOperationCancellation {
    /// Creates an uncancelled account-operation handle.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(AccountOperationCancellationState {
                cancellation: CancellationToken::new(),
                phase: AtomicU8::new(AccountOperationPhase::Active as u8),
            }),
        }
    }

    /// Requests cancellation while the operation remains before credential commit.
    pub fn cancel(&self) {
        if self
            .state
            .phase
            .compare_exchange(
                AccountOperationPhase::Active as u8,
                AccountOperationPhase::Cancelled as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            self.state.cancellation.cancel();
        }
    }

    /// Waits until cancellation wins before credential commit.
    pub async fn cancelled(&self) {
        self.state.cancellation.cancelled().await;
    }

    /// Marks the start of the credential write and rejects an earlier cancellation.
    ///
    /// Repeated calls after the commit transition also succeed.
    ///
    /// # Errors
    ///
    /// Returns [`AccountOperationError::SignInCancelled`] when cancellation
    /// won before credential commit, or
    /// [`AccountOperationError::AccountStateUnavailable`] if the internal
    /// operation phase is invalid.
    pub fn begin_credential_commit(&self) -> Result<(), AccountOperationError> {
        match self.state.phase.compare_exchange(
            AccountOperationPhase::Active as u8,
            AccountOperationPhase::CredentialCommitStarted as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => Ok(()),
            Err(phase_value)
                if phase_value == AccountOperationPhase::CredentialCommitStarted as u8 =>
            {
                Ok(())
            }
            Err(phase_value) if phase_value == AccountOperationPhase::Cancelled as u8 => {
                Err(AccountOperationError::SignInCancelled)
            }
            Err(_) => Err(AccountOperationError::AccountStateUnavailable),
        }
    }
}

impl Default for AccountOperationCancellation {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
struct AccountOperationCancellationState {
    cancellation: CancellationToken,
    phase: AtomicU8,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AccountOperationPhase {
    Active,
    Cancelled,
    CredentialCommitStarted,
}

/// Operations the application needs from a connected-account provider.
pub trait AccountProvider: Send + Sync {
    /// Restores the saved account and identifies its provider, if a credential exists.
    ///
    /// # Errors
    ///
    /// Returns a provider error when the credential cannot be loaded, renewed,
    /// or mapped to an account profile.
    fn restore_account(
        &self,
    ) -> impl Future<Output = Result<Option<ConnectedAccount>, AccountOperationError>> + Send + '_;

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

    /// Connects the selected account and identifies the provider that signed it in.
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
    ) -> impl Future<Output = Result<ConnectedAccount, AccountOperationError>> + Send + '_;

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
    ) -> impl Future<Output = Result<ConnectedAccount, AccountOperationError>> + Send + '_ {
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

    /// Reviews permissions and returns the active account with its provider kind.
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
    ) -> impl Future<Output = Result<ConnectedAccount, AccountOperationError>> + Send + '_;

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

/// Coordinates the product's single-account connection workflow.
pub struct AccountWorkflow<P> {
    provider: P,
    state: Arc<AccountWorkflowState>,
}

impl<P: AccountProvider> AccountWorkflow<P> {
    /// Creates workflows backed by one account provider.
    #[must_use]
    pub fn new(provider: P) -> Self {
        Self {
            provider,
            state: Arc::new(AccountWorkflowState::default()),
        }
    }

    /// Loads the one saved account, if one is available.
    ///
    /// `Ok(None)` means the provider has no saved account to restore.
    ///
    /// # Errors
    ///
    /// Returns a provider error when loading or renewing the saved account
    /// fails, or [`AccountOperationError::AccountOperationInProgress`] when
    /// another account operation is running. Returns
    /// [`AccountOperationError::AccountStateUnavailable`] if local workflow
    /// state cannot be read or updated.
    pub async fn restore_account(&self) -> Result<Option<ConnectedAccount>, AccountOperationError> {
        let operation = self.begin(AccountOperation::Restoring)?;
        let restore_result = self.provider.restore_account().await;
        if let Ok(account) = &restore_result {
            operation.complete(account.as_ref().map(|account| account.profile().clone()))?;
        }
        restore_result
    }

    /// Returns the verified identity that can authorize removal of the account cache.
    ///
    /// This reserves the account workflow while the provider reads the credential.
    ///
    /// # Errors
    ///
    /// Returns [`AccountOperationError::AccountOperationInProgress`] when
    /// another account operation is running, or
    /// [`AccountOperationError::AccountStateUnavailable`] if workflow state
    /// cannot be reserved. Returns
    /// [`AccountOperationError::AccountMismatch`] if saved and active identities
    /// differ, or another provider error when the identity cannot be read.
    pub async fn saved_credential_account_id(
        &self,
    ) -> Result<Option<AccountId>, AccountOperationError> {
        let _operation = self.begin(AccountOperation::ReadingSavedIdentity)?;
        self.provider.saved_credential_account_id().await
    }

    /// Connects an account when no account is active.
    ///
    /// # Errors
    ///
    /// Returns [`AccountOperationError::AccountAlreadyConnected`] when the
    /// workflow already has an account, or
    /// [`AccountOperationError::AccountOperationInProgress`] when another
    /// account operation is running. Returns
    /// [`AccountOperationError::AccountStateUnavailable`] if local workflow
    /// state cannot be read or updated, or
    /// [`AccountOperationError::SignInCancelled`] when the provider or browser
    /// cancels sign-in. Returns
    /// [`AccountOperationError::SavedAccountIdentityUnavailable`] when an
    /// unverified credential blocks replacement, or
    /// [`AccountOperationError::AccountMismatch`] when a verified saved
    /// identity differs from the newly authorized account. Otherwise returns
    /// a provider error.
    ///
    /// Dropping this future releases its operation reservation without
    /// changing the workflow's active account.
    pub async fn connect_account(&self) -> Result<ConnectedAccount, AccountOperationError> {
        self.connect_account_with_cancellation(AccountOperationCancellation::new())
            .await
    }

    /// Connects an account from user-supplied provider details.
    ///
    /// # Errors
    ///
    /// Returns [`AccountOperationError::AccountAlreadyConnected`] when an
    /// account is active, [`AccountOperationError::UnsupportedOperation`]
    /// when the selected provider does not support this request, or another
    /// provider error when the connection cannot be established.
    pub async fn connect_account_with_request(
        &self,
        request: AccountConnectionRequest,
    ) -> Result<ConnectedAccount, AccountOperationError> {
        self.connect_account_with_request_and_cancellation(
            request,
            AccountOperationCancellation::new(),
        )
        .await
    }

    /// Connects an account and keeps the operation result available after a cancellation request.
    ///
    /// The provider orders cancellation against its credential commit using
    /// `cancellation`.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::connect_account`]. If cancellation
    /// wins before credential commit, returns
    /// [`AccountOperationError::SignInCancelled`].
    pub async fn connect_account_with_cancellation(
        &self,
        cancellation: AccountOperationCancellation,
    ) -> Result<ConnectedAccount, AccountOperationError> {
        self.connect_account_with_request_and_cancellation(
            AccountConnectionRequest::Microsoft365,
            cancellation,
        )
        .await
    }

    /// Connects an account and keeps the result available after cancellation is requested.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::connect_account_with_request`]. If
    /// cancellation wins before credential commit, returns
    /// [`AccountOperationError::SignInCancelled`].
    pub async fn connect_account_with_request_and_cancellation(
        &self,
        request: AccountConnectionRequest,
        cancellation: AccountOperationCancellation,
    ) -> Result<ConnectedAccount, AccountOperationError> {
        let operation = self.begin(AccountOperation::Connecting)?;
        let connect_result = self
            .provider
            .connect_account_with_request(request, cancellation)
            .await;
        if let Ok(account) = &connect_result {
            operation.complete(Some(account.profile().clone()))?;
        }
        connect_result
    }

    /// Discovers an Exchange EWS endpoint or continues a confirmed redirect.
    ///
    /// Discovery reserves the account workflow and is available only while no
    /// account is connected. The provider receives a mailbox address and a
    /// confirmation challenge, never account credentials.
    ///
    /// # Errors
    ///
    /// Returns [`AccountOperationError::AccountAlreadyConnected`] when an
    /// account is active, [`AccountOperationError::AccountOperationInProgress`]
    /// when another account operation is running, or a provider error when
    /// discovery fails.
    pub async fn discover_exchange_ews_endpoint(
        &self,
        request: ExchangeEwsDiscoveryRequest,
    ) -> Result<ExchangeEwsDiscoveryResult, AccountOperationError> {
        let _operation = self.begin(AccountOperation::Discovering)?;
        self.provider.discover_exchange_ews_endpoint(request).await
    }

    /// Renews consent while keeping the current account active.
    ///
    /// The provider receives the active account ID and must verify that the
    /// browser flow returns the same identity before replacing credentials.
    /// `request` selects the existing permission set or adds delegated mail
    /// read/write, calendar read/write, or mail send access.
    ///
    /// # Errors
    ///
    /// Returns [`AccountOperationError::AccountNotConnected`] when there is no
    /// active account, [`AccountOperationError::AccountMismatch`] when the
    /// supplied or returned identity differs,
    /// [`AccountOperationError::AccountOperationInProgress`] when another
    /// account operation is running, [`AccountOperationError::AccountStateUnavailable`]
    /// when workflow state cannot be read or updated,
    /// [`AccountOperationError::SignInCancelled`] when authorization is
    /// cancelled, or another provider error when authorization or credential
    /// storage fails.
    pub async fn reauthorize_account(
        &self,
        expected_account_id: &AccountId,
        request: AccountPermissionRequest,
    ) -> Result<ConnectedAccount, AccountOperationError> {
        self.reauthorize_account_with_cancellation(
            expected_account_id,
            request,
            AccountOperationCancellation::new(),
        )
        .await
    }

    /// Renews consent with cancellation ordered against credential persistence.
    ///
    /// `request` selects the delegated permissions to request. `cancellation`
    /// controls the provider operation through the credential commit boundary.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::reauthorize_account`].
    pub async fn reauthorize_account_with_cancellation(
        &self,
        expected_account_id: &AccountId,
        request: AccountPermissionRequest,
        cancellation: AccountOperationCancellation,
    ) -> Result<ConnectedAccount, AccountOperationError> {
        let operation = self.begin(AccountOperation::Reauthorizing)?;
        self.ensure_active_account_matches(expected_account_id)?;
        let reauthorization_result = self
            .provider
            .reauthorize_account(expected_account_id, request, cancellation)
            .await;
        if let Ok(account) = &reauthorization_result {
            if account.profile().id() != expected_account_id {
                return Err(AccountOperationError::AccountMismatch);
            }
            operation.complete(Some(account.profile().clone()))?;
        }
        reauthorization_result
    }

    /// Removes the provider's saved credential and clears the active profile.
    ///
    /// # Errors
    ///
    /// Returns [`AccountOperationError::SavedAccountIdentityUnavailable`]
    /// when the saved credential has no Graph-verified identity,
    /// [`AccountOperationError::AccountMismatch`] when the saved and active
    /// identities differ, or another provider error when the credential
    /// cannot be removed. Returns
    /// [`AccountOperationError::AccountOperationInProgress`] when another
    /// account operation is running, or
    /// [`AccountOperationError::AccountStateUnavailable`] if local workflow
    /// state cannot be read or updated.
    pub async fn sign_out(&self) -> Result<(), AccountOperationError> {
        let operation = self.begin(AccountOperation::SigningOut)?;
        self.provider.sign_out().await?;
        operation.complete(None)
    }

    fn begin(
        &self,
        operation: AccountOperation,
    ) -> Result<AccountOperationGuard, AccountOperationError> {
        self.reserve_operation(operation)?;

        Ok(AccountOperationGuard {
            state: Arc::clone(&self.state),
            operation,
            completed: false,
        })
    }

    fn reserve_operation(&self, operation: AccountOperation) -> Result<(), AccountOperationError> {
        if self
            .state
            .operation
            .compare_exchange(
                AccountOperation::Idle as u8,
                operation as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return Err(AccountOperationError::AccountOperationInProgress);
        }
        if matches!(
            operation,
            AccountOperation::Connecting | AccountOperation::Discovering
        ) {
            self.ensure_no_active_account(operation)?;
        }
        Ok(())
    }

    fn ensure_no_active_account(
        &self,
        operation: AccountOperation,
    ) -> Result<(), AccountOperationError> {
        let has_active_account = match self.state.active_account.lock() {
            Ok(active_account) => active_account.is_some(),
            Err(_) => {
                self.release_operation(operation);
                return Err(AccountOperationError::AccountStateUnavailable);
            }
        };
        if has_active_account {
            self.release_operation(operation);
            return Err(AccountOperationError::AccountAlreadyConnected);
        }
        Ok(())
    }

    fn ensure_active_account_matches(
        &self,
        expected_account_id: &AccountId,
    ) -> Result<(), AccountOperationError> {
        let active_account = self
            .state
            .active_account
            .lock()
            .map_err(|_| AccountOperationError::AccountStateUnavailable)?;
        let Some(active_account) = active_account.as_ref() else {
            return Err(AccountOperationError::AccountNotConnected);
        };
        if active_account.id() != expected_account_id {
            return Err(AccountOperationError::AccountMismatch);
        }
        Ok(())
    }

    fn release_operation(&self, operation: AccountOperation) {
        let _ = self.state.operation.compare_exchange(
            operation as u8,
            AccountOperation::Idle as u8,
            Ordering::Release,
            Ordering::Relaxed,
        );
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum AccountOperation {
    #[default]
    Idle,
    Restoring,
    ReadingSavedIdentity,
    Discovering,
    Connecting,
    Reauthorizing,
    SigningOut,
}

struct AccountWorkflowState {
    active_account: Mutex<Option<AccountProfile>>,
    operation: AtomicU8,
}

impl Default for AccountWorkflowState {
    fn default() -> Self {
        Self {
            active_account: Mutex::new(None),
            operation: AtomicU8::new(AccountOperation::Idle as u8),
        }
    }
}

struct AccountOperationGuard {
    state: Arc<AccountWorkflowState>,
    operation: AccountOperation,
    completed: bool,
}

impl AccountOperationGuard {
    fn complete(
        mut self,
        active_account: Option<AccountProfile>,
    ) -> Result<(), AccountOperationError> {
        let mut state = self
            .state
            .active_account
            .lock()
            .map_err(|_| AccountOperationError::AccountStateUnavailable)?;
        *state = active_account;
        drop(state);
        self.state
            .operation
            .store(AccountOperation::Idle as u8, Ordering::Release);
        self.completed = true;
        Ok(())
    }
}

impl Drop for AccountOperationGuard {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        let _ = self.state.operation.compare_exchange(
            self.operation as u8,
            AccountOperation::Idle as u8,
            Ordering::Release,
            Ordering::Relaxed,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::{
        future::{Future, ready},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };

    use einfach_mail_domain::{AccountId, AccountProfile};

    use super::{
        AccountConnectionRequest, AccountOperationCancellation, AccountOperationError,
        AccountProvider, AccountProviderKind, AccountWorkflow, ConnectedAccount,
        ExchangeEwsConnectionInput,
    };

    #[test]
    fn ews_connection_debug_redacts_endpoint_identity_and_password() {
        let input = ExchangeEwsConnectionInput::new(
            "https://exchange.example.test/EWS/Exchange.asmx".to_owned(),
            "mailbox-marker@example.test".to_owned(),
            "DOMAIN\\username-marker".to_owned(),
            "password-marker".to_owned(),
        );
        let input_debug = format!("{input:?}");
        let request_debug = format!("{:?}", AccountConnectionRequest::ExchangeEws(input));

        for marker in [
            "exchange.example.test",
            "mailbox-marker",
            "username-marker",
            "password-marker",
        ] {
            assert!(!input_debug.contains(marker));
            assert!(!request_debug.contains(marker));
        }
    }

    #[tokio::test]
    async fn cancellation_before_credential_commit_rejects_the_commit() {
        let cancellation = AccountOperationCancellation::new();
        cancellation.cancel();

        assert_eq!(
            cancellation.begin_credential_commit(),
            Err(AccountOperationError::SignInCancelled)
        );
        assert!(tokio::select! {
            biased;
            () = cancellation.cancelled() => true,
            () = tokio::task::yield_now() => false,
        });
    }

    #[tokio::test]
    async fn credential_commit_prevents_a_later_cancellation() {
        let cancellation = AccountOperationCancellation::new();
        cancellation
            .begin_credential_commit()
            .expect("uncancelled operation can start its credential commit");
        cancellation.cancel();

        assert_eq!(cancellation.begin_credential_commit(), Ok(()));
        assert!(!tokio::select! {
            biased;
            () = cancellation.cancelled() => true,
            () = tokio::task::yield_now() => false,
        });
    }

    fn synthetic_profile() -> AccountProfile {
        AccountProfile::try_new(
            AccountId::try_from("synthetic-account".to_owned()).expect("synthetic ID is non-empty"),
            Some("Ada Example".to_owned()),
            None,
            Some("ada@example.test".to_owned()),
        )
        .expect("synthetic identity is available")
    }

    #[derive(Clone, Default)]
    struct FakeProvider {
        fail_next_sign_out: Arc<AtomicBool>,
        pause_sign_out: Arc<AtomicBool>,
        restore_missing: Arc<AtomicBool>,
    }

    impl AccountProvider for FakeProvider {
        fn restore_account(
            &self,
        ) -> impl Future<Output = Result<Option<ConnectedAccount>, AccountOperationError>> + Send + '_
        {
            let restore_missing = self.restore_missing.load(Ordering::SeqCst);
            ready(Ok((!restore_missing).then(|| {
                ConnectedAccount::new(synthetic_profile(), AccountProviderKind::MicrosoftGraph)
            })))
        }

        fn saved_credential_account_id(
            &self,
        ) -> impl Future<Output = Result<Option<AccountId>, AccountOperationError>> + Send + '_
        {
            ready(Ok(Some(synthetic_profile().id().clone())))
        }

        fn connect_account(
            &self,
            _: super::AccountOperationCancellation,
        ) -> impl Future<Output = Result<ConnectedAccount, AccountOperationError>> + Send + '_
        {
            ready(Ok(ConnectedAccount::new(
                synthetic_profile(),
                AccountProviderKind::MicrosoftGraph,
            )))
        }

        fn reauthorize_account(
            &self,
            _: &AccountId,
            _: super::AccountPermissionRequest,
            _: super::AccountOperationCancellation,
        ) -> impl Future<Output = Result<ConnectedAccount, AccountOperationError>> + Send + '_
        {
            ready(Ok(ConnectedAccount::new(
                synthetic_profile(),
                AccountProviderKind::MicrosoftGraph,
            )))
        }

        fn sign_out(&self) -> impl Future<Output = Result<(), AccountOperationError>> + Send + '_ {
            let fail_next_sign_out = Arc::clone(&self.fail_next_sign_out);
            let pause_sign_out = Arc::clone(&self.pause_sign_out);
            async move {
                if pause_sign_out.load(Ordering::SeqCst) {
                    std::future::pending::<()>().await;
                }
                if fail_next_sign_out.swap(false, Ordering::SeqCst) {
                    Err(AccountOperationError::CredentialStoreUnavailable)
                } else {
                    Ok(())
                }
            }
        }
    }

    #[tokio::test]
    async fn restore_without_saved_account_clears_active_account() {
        let provider = FakeProvider::default();
        let restore_missing = Arc::clone(&provider.restore_missing);
        let workflow = AccountWorkflow::new(provider);

        workflow
            .connect_account()
            .await
            .expect("synthetic account connection succeeds");
        restore_missing.store(true, Ordering::SeqCst);

        assert_eq!(workflow.restore_account().await, Ok(None));
        assert_eq!(
            workflow.connect_account().await,
            Ok(ConnectedAccount::new(
                synthetic_profile(),
                AccountProviderKind::MicrosoftGraph,
            ))
        );
    }

    struct PendingProvider;

    impl AccountProvider for PendingProvider {
        fn restore_account(
            &self,
        ) -> impl Future<Output = Result<Option<ConnectedAccount>, AccountOperationError>> + Send + '_
        {
            ready(Ok(None))
        }

        fn saved_credential_account_id(
            &self,
        ) -> impl Future<Output = Result<Option<AccountId>, AccountOperationError>> + Send + '_
        {
            ready(Ok(None))
        }

        fn connect_account(
            &self,
            _: super::AccountOperationCancellation,
        ) -> impl Future<Output = Result<ConnectedAccount, AccountOperationError>> + Send + '_
        {
            std::future::pending()
        }

        fn reauthorize_account(
            &self,
            _: &AccountId,
            _: super::AccountPermissionRequest,
            _: super::AccountOperationCancellation,
        ) -> impl Future<Output = Result<ConnectedAccount, AccountOperationError>> + Send + '_
        {
            std::future::pending()
        }

        fn sign_out(&self) -> impl Future<Output = Result<(), AccountOperationError>> + Send + '_ {
            ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn keeps_one_connected_account_in_the_workflow() {
        let workflow = AccountWorkflow::new(FakeProvider::default());
        workflow
            .connect_account()
            .await
            .expect("synthetic profile is available");

        assert_eq!(
            workflow.connect_account().await,
            Err(AccountOperationError::AccountAlreadyConnected)
        );
    }

    #[tokio::test]
    async fn saved_identity_read_is_serialized_with_account_operations() {
        let workflow = Arc::new(AccountWorkflow::new(PendingProvider));
        let pending_workflow = Arc::clone(&workflow);
        let connection = tokio::spawn(async move { pending_workflow.connect_account().await });
        tokio::task::yield_now().await;

        assert_eq!(
            workflow.saved_credential_account_id().await,
            Err(AccountOperationError::AccountOperationInProgress)
        );

        connection.abort();
        let _ = connection.await;
        assert_eq!(workflow.saved_credential_account_id().await, Ok(None));
    }

    #[tokio::test]
    async fn does_not_connect_when_restore_finds_an_active_account() {
        let workflow = AccountWorkflow::new(FakeProvider::default());

        assert_eq!(
            workflow.restore_account().await,
            Ok(Some(ConnectedAccount::new(
                synthetic_profile(),
                AccountProviderKind::MicrosoftGraph,
            )))
        );
        assert_eq!(
            workflow.connect_account().await,
            Err(AccountOperationError::AccountAlreadyConnected)
        );
    }

    #[tokio::test]
    async fn removes_the_saved_account_credential() {
        let workflow = AccountWorkflow::new(FakeProvider::default());

        assert_eq!(workflow.sign_out().await, Ok(()));
        assert!(workflow.connect_account().await.is_ok());
    }

    #[tokio::test]
    async fn failed_sign_out_keeps_the_account_for_retry() {
        let provider = FakeProvider::default();
        provider.fail_next_sign_out.store(true, Ordering::SeqCst);
        let workflow = AccountWorkflow::new(provider);
        workflow
            .connect_account()
            .await
            .expect("synthetic profile is available");

        assert_eq!(
            workflow.sign_out().await,
            Err(AccountOperationError::CredentialStoreUnavailable)
        );
        assert_eq!(
            workflow.connect_account().await,
            Err(AccountOperationError::AccountAlreadyConnected)
        );
        assert_eq!(workflow.sign_out().await, Ok(()));
        assert!(workflow.connect_account().await.is_ok());
    }

    #[tokio::test]
    async fn cancelling_sign_out_releases_its_reservation_and_keeps_the_account() {
        let provider = FakeProvider::default();
        provider.pause_sign_out.store(true, Ordering::SeqCst);
        let workflow = AccountWorkflow::new(provider.clone());
        let profile = workflow
            .connect_account()
            .await
            .expect("synthetic profile is available");

        {
            let sign_out = workflow.sign_out();
            tokio::pin!(sign_out);
            let completed = tokio::select! {
                biased;
                result = sign_out.as_mut() => Some(result),
                () = tokio::task::yield_now() => None,
            };
            assert!(completed.is_none(), "synthetic sign-out remains pending");
        }

        provider.pause_sign_out.store(false, Ordering::SeqCst);
        assert_eq!(
            workflow
                .reauthorize_account(
                    profile.profile().id(),
                    super::AccountPermissionRequest::Current,
                )
                .await,
            Ok(profile)
        );
        assert_eq!(workflow.sign_out().await, Ok(()));
    }

    #[tokio::test]
    async fn renews_consent_without_changing_the_active_account() {
        let workflow = AccountWorkflow::new(FakeProvider::default());
        let profile = workflow
            .connect_account()
            .await
            .expect("synthetic profile is available");

        let renewed = workflow
            .reauthorize_account(
                profile.profile().id(),
                super::AccountPermissionRequest::Current,
            )
            .await
            .expect("synthetic consent renewal succeeds");

        assert_eq!(renewed, profile);
        assert_eq!(
            workflow.connect_account().await,
            Err(AccountOperationError::AccountAlreadyConnected)
        );
    }

    #[tokio::test]
    async fn permission_renewal_requires_the_active_account_identity() {
        let workflow = AccountWorkflow::new(FakeProvider::default());
        let expected_account =
            AccountId::try_from("synthetic-account".to_owned()).expect("ID is valid");
        assert_eq!(
            workflow
                .reauthorize_account(&expected_account, super::AccountPermissionRequest::Current)
                .await,
            Err(AccountOperationError::AccountNotConnected)
        );

        workflow
            .connect_account()
            .await
            .expect("synthetic profile is available");
        let other_account = AccountId::try_from("other-account".to_owned()).expect("ID is valid");
        assert_eq!(
            workflow
                .reauthorize_account(
                    &other_account,
                    super::AccountPermissionRequest::MailReadWrite
                )
                .await,
            Err(AccountOperationError::AccountMismatch)
        );
    }

    #[tokio::test]
    async fn cancelling_a_connection_releases_its_operation_reservation() {
        let workflow = AccountWorkflow::new(PendingProvider);
        {
            let connect_future = workflow.connect_account();
            tokio::pin!(connect_future);
            let completed = tokio::select! {
                biased;
                connect_result = connect_future.as_mut() => Some(connect_result),
                () = tokio::task::yield_now() => None,
            };
            assert!(completed.is_none(), "synthetic connection remains pending");
        }

        assert_eq!(workflow.sign_out().await, Ok(()));
    }
}
