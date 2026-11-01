//! Account connection workflows and their provider port.

use std::{
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
};

use einfach_mail_domain::{AccountId, AccountProfile};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

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
    /// No account is active for a permission update.
    #[error("no account is connected")]
    AccountNotConnected,
    /// The account selected during permission renewal does not match the active account.
    #[error("the selected account does not match the connected account")]
    AccountMismatch,
    /// Another account operation is still running.
    #[error("another account operation is still running")]
    AccountOperationInProgress,
    /// The workflow could not read or update its local account state.
    #[error("account workflow state is unavailable")]
    AccountStateUnavailable,
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
    /// Implementations must call [`AccountOperationCancellation::begin_credential_commit`]
    /// immediately before storing the refresh credential.
    ///
    /// # Errors
    ///
    /// Returns [`AccountOperationError::SignInCancelled`] when cancellation
    /// wins before credential commit. Returns another provider error when
    /// authorization, profile loading, or credential storage fails.
    fn connect_account(
        &self,
        cancellation: AccountOperationCancellation,
    ) -> impl Future<Output = Result<AccountProfile, AccountOperationError>> + Send + '_;

    /// Requests the current delegated permissions again for the active account.
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
        cancellation: AccountOperationCancellation,
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
    /// state cannot be read or updated, or
    /// [`AccountOperationError::SignInCancelled`] when the provider or browser
    /// cancels sign-in. Otherwise returns a provider error.
    ///
    /// Dropping this future releases its operation reservation without
    /// changing the workflow's active account.
    pub async fn connect_account(&self) -> Result<AccountProfile, AccountOperationError> {
        self.connect_account_with_cancellation(AccountOperationCancellation::new())
            .await
    }

    /// Connects an account and keeps the operation result available after a
    /// cancellation request.
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
    ) -> Result<AccountProfile, AccountOperationError> {
        let operation = self.begin(AccountOperation::Connecting)?;
        let connect_result = self.provider.connect_account(cancellation).await;
        if let Ok(account) = &connect_result {
            operation.complete(Some(account.clone()))?;
        }
        connect_result
    }

    /// Renews consent while keeping the current account active.
    ///
    /// The provider receives the active account ID and must verify that the
    /// browser flow returns the same identity before replacing credentials.
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
    ) -> Result<AccountProfile, AccountOperationError> {
        self.reauthorize_account_with_cancellation(
            expected_account_id,
            AccountOperationCancellation::new(),
        )
        .await
    }

    /// Renews consent with cancellation ordered against credential persistence.
    ///
    /// `cancellation` controls the provider operation through the credential
    /// commit boundary.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::reauthorize_account`].
    pub async fn reauthorize_account_with_cancellation(
        &self,
        expected_account_id: &AccountId,
        cancellation: AccountOperationCancellation,
    ) -> Result<AccountProfile, AccountOperationError> {
        let operation = self.begin(AccountOperation::Reauthorizing)?;
        self.ensure_active_account_matches(expected_account_id)?;
        let reauthorization_result = self
            .provider
            .reauthorize_account(expected_account_id, cancellation)
            .await;
        if let Ok(profile) = &reauthorization_result {
            if profile.id() != expected_account_id {
                return Err(AccountOperationError::AccountMismatch);
            }
            operation.complete(Some(profile.clone()))?;
        }
        reauthorization_result
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
        AccountOperationCancellation, AccountOperationError, AccountProvider, AccountWorkflow,
    };

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
    }

    impl AccountProvider for FakeProvider {
        fn restore_account(
            &self,
        ) -> impl Future<Output = Result<Option<AccountProfile>, AccountOperationError>> + Send + '_
        {
            ready(Ok(Some(synthetic_profile())))
        }

        fn connect_account(
            &self,
            _: super::AccountOperationCancellation,
        ) -> impl Future<Output = Result<AccountProfile, AccountOperationError>> + Send + '_
        {
            ready(Ok(synthetic_profile()))
        }

        fn reauthorize_account(
            &self,
            _: &AccountId,
            _: super::AccountOperationCancellation,
        ) -> impl Future<Output = Result<AccountProfile, AccountOperationError>> + Send + '_
        {
            ready(Ok(synthetic_profile()))
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
            _: super::AccountOperationCancellation,
        ) -> impl Future<Output = Result<AccountProfile, AccountOperationError>> + Send + '_
        {
            std::future::pending()
        }

        fn reauthorize_account(
            &self,
            _: &AccountId,
            _: super::AccountOperationCancellation,
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
    async fn does_not_connect_when_restore_finds_an_active_account() {
        let workflow = AccountWorkflow::new(FakeProvider::default());

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
            workflow.reauthorize_account(profile.id()).await,
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
            .reauthorize_account(profile.id())
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
            workflow.reauthorize_account(&expected_account).await,
            Err(AccountOperationError::AccountNotConnected)
        );

        workflow
            .connect_account()
            .await
            .expect("synthetic profile is available");
        let other_account = AccountId::try_from("other-account".to_owned()).expect("ID is valid");
        assert_eq!(
            workflow.reauthorize_account(&other_account).await,
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
