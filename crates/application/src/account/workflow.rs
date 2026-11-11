use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU8, Ordering},
};

use einfach_mail_domain::{AccountId, AccountProfile};
use tokio_util::sync::CancellationToken;

use super::provider::{
    AccountConnectionRequest, AccountOperationError, AccountPermissionRequest, AccountProvider,
    ConnectedAccount, ExchangeEwsDiscoveryRequest, ExchangeEwsDiscoveryResult,
};

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
