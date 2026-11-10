use std::{
    future::{Future, ready},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use einfach_mail_domain::{AccountId, AccountProfile};

use super::{
    AccountConnectionRequest, AccountOperationCancellation, AccountOperationError, AccountProvider,
    AccountWorkflow, ExchangeEwsConnectionInput,
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
}

impl AccountProvider for FakeProvider {
    fn restore_account(
        &self,
    ) -> impl Future<Output = Result<Option<AccountProfile>, AccountOperationError>> + Send + '_
    {
        ready(Ok(Some(synthetic_profile())))
    }

    fn saved_credential_account_id(
        &self,
    ) -> impl Future<Output = Result<Option<AccountId>, AccountOperationError>> + Send + '_ {
        ready(Ok(Some(synthetic_profile().id().clone())))
    }

    fn connect_account(
        &self,
        _: super::AccountOperationCancellation,
    ) -> impl Future<Output = Result<AccountProfile, AccountOperationError>> + Send + '_ {
        ready(Ok(synthetic_profile()))
    }

    fn reauthorize_account(
        &self,
        _: &AccountId,
        _: super::AccountPermissionRequest,
        _: super::AccountOperationCancellation,
    ) -> impl Future<Output = Result<AccountProfile, AccountOperationError>> + Send + '_ {
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

    fn saved_credential_account_id(
        &self,
    ) -> impl Future<Output = Result<Option<AccountId>, AccountOperationError>> + Send + '_ {
        ready(Ok(None))
    }

    fn connect_account(
        &self,
        _: super::AccountOperationCancellation,
    ) -> impl Future<Output = Result<AccountProfile, AccountOperationError>> + Send + '_ {
        std::future::pending()
    }

    fn reauthorize_account(
        &self,
        _: &AccountId,
        _: super::AccountPermissionRequest,
        _: super::AccountOperationCancellation,
    ) -> impl Future<Output = Result<AccountProfile, AccountOperationError>> + Send + '_ {
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
        workflow
            .reauthorize_account(profile.id(), super::AccountPermissionRequest::Current)
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
        .reauthorize_account(profile.id(), super::AccountPermissionRequest::Current)
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
