//! Account connection workflows and their provider port.

use std::{
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
};

use einfach_mail_domain::AccountProfile;
use thiserror::Error;

/// A user-safe category for an account workflow failure.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AccountOperationError {
    /// The user cancelled browser sign-in.
    #[error("sign-in was cancelled")]
    SignInCancelled,
    /// The authorization server did not grant the requested access.
    #[error("access was not granted")]
    AccessDenied,
    /// Microsoft rejected the access token used for a Graph profile request.
    #[error("the access token was rejected")]
    AccessTokenRejected,
    /// A stored refresh token can no longer restore the account.
    #[error("saved sign-in has expired")]
    SignInExpired,
    /// The Linux credential service could not read, store, or remove a token.
    #[error("the Linux credential service is unavailable")]
    CredentialStoreUnavailable,
    /// Microsoft Graph returned an incomplete account profile.
    #[error("Microsoft returned an incomplete account profile")]
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
    /// Another account operation is still running.
    #[error("another account operation is still running")]
    AccountOperationInProgress,
    /// The workflow could not read or update its local account state.
    #[error("account workflow state is unavailable")]
    AccountStateUnavailable,
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

    /// Connects an account through the provider's delegated sign-in flow.
    ///
    /// # Errors
    ///
    /// Returns a provider error when authorization, profile loading, or
    /// credential storage fails.
    fn connect_account(
        &self,
    ) -> impl Future<Output = Result<AccountProfile, AccountOperationError>> + Send + '_;

    /// Removes the provider's locally saved refresh credential.
    ///
    /// # Errors
    ///
    /// Returns a provider error when the credential cannot be removed.
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
    pub async fn restore_account(&self) -> Result<Option<AccountProfile>, AccountOperationError> {
        let operation = self.begin(AccountOperation::Restoring)?;
        let restore_result = self.provider.restore_account().await;
        if let Ok(account) = &restore_result {
            operation.complete(account.clone())?;
        }
        restore_result
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
    /// state cannot be read or updated. Otherwise returns a provider error.
    ///
    /// Dropping this future releases its operation reservation without
    /// changing the workflow's active account.
    pub async fn connect_account(&self) -> Result<AccountProfile, AccountOperationError> {
        let operation = self.begin(AccountOperation::Connecting)?;
        let connect_result = self.provider.connect_account().await;
        if let Ok(account) = &connect_result {
            operation.complete(Some(account.clone()))?;
        }
        connect_result
    }

    /// Removes the provider's saved credential and clears the active profile.
    ///
    /// # Errors
    ///
    /// Returns a provider error when the credential cannot be removed, or
    /// [`AccountOperationError::AccountOperationInProgress`] when another
    /// account operation is running. Returns
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
        if operation == AccountOperation::Connecting {
            self.ensure_no_active_account()?;
        }
        Ok(())
    }

    fn ensure_no_active_account(&self) -> Result<(), AccountOperationError> {
        let has_active_account = match self.state.active_account.lock() {
            Ok(active_account) => active_account.is_some(),
            Err(_) => {
                self.release_operation(AccountOperation::Connecting);
                return Err(AccountOperationError::AccountStateUnavailable);
            }
        };
        if has_active_account {
            self.release_operation(AccountOperation::Connecting);
            return Err(AccountOperationError::AccountAlreadyConnected);
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
    Connecting,
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
    use std::future::{Future, ready};

    use einfach_mail_domain::{AccountId, AccountProfile};

    use super::{AccountOperationError, AccountProvider, AccountWorkflow};

    fn synthetic_profile() -> AccountProfile {
        AccountProfile::try_new(
            AccountId::try_from("synthetic-account".to_owned()).expect("synthetic ID is non-empty"),
            Some("Ada Example".to_owned()),
            None,
            Some("ada@example.test".to_owned()),
        )
        .expect("synthetic identity is available")
    }

    #[derive(Clone)]
    struct FakeProvider;

    impl AccountProvider for FakeProvider {
        fn restore_account(
            &self,
        ) -> impl Future<Output = Result<Option<AccountProfile>, AccountOperationError>> + Send + '_
        {
            ready(Ok(Some(synthetic_profile())))
        }

        fn connect_account(
            &self,
        ) -> impl Future<Output = Result<AccountProfile, AccountOperationError>> + Send + '_
        {
            ready(Ok(synthetic_profile()))
        }

        fn sign_out(&self) -> impl Future<Output = Result<(), AccountOperationError>> + Send + '_ {
            ready(Ok(()))
        }
    }

    struct PendingProvider;

    impl AccountProvider for PendingProvider {
        fn restore_account(
            &self,
        ) -> impl Future<Output = Result<Option<AccountProfile>, AccountOperationError>> + Send + '_
        {
            ready(Ok(None))
        }

        fn connect_account(
            &self,
        ) -> impl Future<Output = Result<AccountProfile, AccountOperationError>> + Send + '_
        {
            std::future::pending()
        }

        fn sign_out(&self) -> impl Future<Output = Result<(), AccountOperationError>> + Send + '_ {
            ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn keeps_one_connected_account_in_the_workflow() {
        let workflow = AccountWorkflow::new(FakeProvider);
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
    async fn does_not_connect_when_restore_finds_an_active_account() {
        let workflow = AccountWorkflow::new(FakeProvider);

        assert_eq!(
            workflow.restore_account().await,
            Ok(Some(synthetic_profile()))
        );
        assert_eq!(
            workflow.connect_account().await,
            Err(AccountOperationError::AccountAlreadyConnected)
        );
    }

    #[tokio::test]
    async fn removes_the_saved_account_credential() {
        let workflow = AccountWorkflow::new(FakeProvider);

        assert_eq!(workflow.sign_out().await, Ok(()));
        assert!(workflow.connect_account().await.is_ok());
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
