use std::{future::Future, sync::Arc};

use einfach_mail_application::{
    AccountConnectionRequest, AccountOperationCancellation, AccountOperationError,
    AccountPermissionRequest, AccountProvider, AccountProviderKind, ConnectedAccount,
    ExchangeEwsAccountSettingsStore, ExchangeEwsConnectionInput, ExchangeEwsDiscoveryRequest,
    ExchangeEwsDiscoveryResult,
};
use einfach_mail_domain::AccountId;
use einfach_mail_provider_runtime as worker;
use secrecy::SecretString;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use super::{
    ExchangeEwsProvider,
    mail::collect_folders,
    session::{EwsSession, profile_for, validate_settings},
};
use crate::auth::{MAX_CREDENTIAL_BYTES, delete_password, load_password, save_password};

mod autodiscover;
pub(super) use self::autodiscover::EwsAutodiscover;
impl<S> ExchangeEwsProvider<S>
where
    S: ExchangeEwsAccountSettingsStore + 'static,
{
    async fn connect_ews_account(
        settings_store: Arc<S>,
        session_store: Arc<RwLock<Option<Arc<EwsSession>>>>,
        input: ExchangeEwsConnectionInput,
        cancellation: AccountOperationCancellation,
    ) -> Result<ConnectedAccount, AccountOperationError> {
        let settings = validate_settings(&input)?;
        let password = SecretString::from(input.expose_password().to_owned());
        let credential_bytes = Zeroizing::new(input.expose_password().as_bytes().to_vec());
        if credential_bytes.is_empty() || credential_bytes.len() > MAX_CREDENTIAL_BYTES {
            return Err(AccountOperationError::InvalidConnectionDetails);
        }

        let mut session = EwsSession::new(settings.clone(), password)?;
        let folder_collection = tokio::select! {
            () = cancellation.cancelled() => return Err(AccountOperationError::SignInCancelled),
            result = collect_folders(&session) => result.map_err(AccountOperationError::from)?,
        };
        session.profile = profile_for(&settings, folder_collection.server_version)
            .map_err(AccountOperationError::from)?;
        cancellation.begin_credential_commit()?;

        blocking::unblock(move || save_password(&credential_bytes))
            .await
            .map_err(AccountOperationError::from)?;
        if settings_store.save(settings.clone()).await.is_err() {
            blocking::unblock(delete_password)
                .await
                .map_err(AccountOperationError::from)?;
            return Err(AccountOperationError::AccountStateUnavailable);
        }

        let profile = session.profile.clone();
        *session_store.write().await = Some(Arc::new(session));
        Ok(ConnectedAccount::new(
            profile,
            AccountProviderKind::ExchangeEws,
        ))
    }
}

impl<S> AccountProvider for ExchangeEwsProvider<S>
where
    S: ExchangeEwsAccountSettingsStore + 'static,
{
    fn restore_account(
        &self,
    ) -> impl Future<Output = Result<Option<ConnectedAccount>, AccountOperationError>> + Send + '_
    {
        let settings_store = Arc::clone(&self.settings_store);
        let session_store = Arc::clone(&self.session);
        worker::run(move |_cancellation: CancellationToken| async move {
            let Some(settings) = settings_store.load().await? else {
                blocking::unblock(delete_password)
                    .await
                    .map_err(AccountOperationError::from)?;
                *session_store.write().await = None;
                return Ok(None);
            };
            let password = blocking::unblock(load_password).await?;
            let Some(password) = password else {
                *session_store.write().await = None;
                return Ok(None);
            };
            let mut session = EwsSession::new(settings.clone(), password)?;
            let folder_collection = collect_folders(&session)
                .await
                .map_err(AccountOperationError::from)?;
            session.profile = profile_for(&settings, folder_collection.server_version)?;
            let profile = session.profile.clone();
            *session_store.write().await = Some(Arc::new(session));
            Ok(Some(ConnectedAccount::new(
                profile,
                AccountProviderKind::ExchangeEws,
            )))
        })
    }

    fn saved_credential_account_id(
        &self,
    ) -> impl Future<Output = Result<Option<AccountId>, AccountOperationError>> + Send + '_ {
        let settings_store = Arc::clone(&self.settings_store);
        worker::run(move |_cancellation: CancellationToken| async move {
            let Some(settings) = settings_store.load().await? else {
                blocking::unblock(delete_password)
                    .await
                    .map_err(AccountOperationError::from)?;
                return Ok(None);
            };
            let password = blocking::unblock(load_password).await?;
            if password.is_none() {
                return Ok(None);
            }
            Ok(Some(profile_for(&settings, None)?.id().clone()))
        })
    }

    fn connect_account(
        &self,
        _cancellation: AccountOperationCancellation,
    ) -> impl Future<Output = Result<ConnectedAccount, AccountOperationError>> + Send + '_ {
        std::future::ready(Err(AccountOperationError::UnsupportedOperation))
    }

    fn connect_account_with_request(
        &self,
        request: AccountConnectionRequest,
        cancellation: AccountOperationCancellation,
    ) -> impl Future<Output = Result<ConnectedAccount, AccountOperationError>> + Send + '_ {
        let settings_store = Arc::clone(&self.settings_store);
        let session_store = Arc::clone(&self.session);
        let commit_cancellation = cancellation.clone();
        async move {
            match request {
                AccountConnectionRequest::ExchangeEws(input) => {
                    worker::run_with_account_cancellation(
                        move |_worker_cancellation: CancellationToken| async move {
                            Self::connect_ews_account(
                                settings_store,
                                session_store,
                                input,
                                commit_cancellation,
                            )
                            .await
                        },
                        cancellation,
                    )
                    .await
                }
                AccountConnectionRequest::Microsoft365 => {
                    Err(AccountOperationError::UnsupportedOperation)
                }
            }
        }
    }

    fn discover_exchange_ews_endpoint(
        &self,
        request: ExchangeEwsDiscoveryRequest,
    ) -> impl Future<Output = Result<ExchangeEwsDiscoveryResult, AccountOperationError>> + Send + '_
    {
        let autodiscover = Arc::clone(&self.autodiscover);
        worker::run(move |_cancellation: CancellationToken| async move {
            autodiscover
                .discover(request)
                .await
                .map_err(AccountOperationError::from)
        })
    }

    fn reauthorize_account(
        &self,
        _expected_account_id: &AccountId,
        _request: AccountPermissionRequest,
        _cancellation: AccountOperationCancellation,
    ) -> impl Future<Output = Result<ConnectedAccount, AccountOperationError>> + Send + '_ {
        std::future::ready(Err(AccountOperationError::UnsupportedOperation))
    }

    fn sign_out(&self) -> impl Future<Output = Result<(), AccountOperationError>> + Send + '_ {
        let settings_store = Arc::clone(&self.settings_store);
        let session_store = Arc::clone(&self.session);
        worker::run(move |_cancellation: CancellationToken| async move {
            let previous = settings_store.load().await?;
            settings_store.delete().await?;
            if let Err(error) = blocking::unblock(delete_password).await {
                if let Some(settings) = previous {
                    settings_store
                        .save(settings)
                        .await
                        .map_err(AccountOperationError::from)?;
                }
                return Err(AccountOperationError::from(error));
            }
            *session_store.write().await = None;
            Ok(())
        })
    }
}
