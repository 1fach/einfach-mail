use std::{future::Future, sync::Arc};

use huskarl::token::{AccessToken, RefreshToken};
use tokio_util::sync::CancellationToken;

use crate::{
    auth::{credential::CredentialIdentity, identity},
    error::GraphError,
    transport::build_client,
};
use einfach_mail_application::{
    AccountOperationCancellation, AccountOperationError, AccountPermissionRequest, AccountProvider,
};
use einfach_mail_domain::{AccountId, AccountProfile};
use einfach_mail_provider_runtime as worker;

use super::{MicrosoftGraphProvider, ProviderConfig, ProviderSession};

struct AuthorizedAccount {
    profile: AccountProfile,
    refresh_token: RefreshToken,
    access_token: Arc<AccessToken>,
}

impl AccountProvider for MicrosoftGraphProvider {
    /// Renews the saved sign-in and loads the account profile.
    ///
    /// # Errors
    ///
    /// Returns a credential-store error when Secret Service cannot read or
    /// update the refresh token, an expired-sign-in error when Microsoft
    /// rejects it, or a service/profile error when refresh or Graph fails.
    /// Returns [`AccountOperationError::AccountAlreadyConnected`] when this
    /// provider already has an active account session.
    fn restore_account(
        &self,
    ) -> impl Future<
        Output = Result<Option<einfach_mail_domain::AccountProfile>, AccountOperationError>,
    > + Send
    + '_ {
        let config = Arc::clone(&self.config);

        worker::run(move |_cancellation: CancellationToken| async move {
            let mut session = config.session.lock().await;
            if session.is_some() {
                return Err(AccountOperationError::AccountAlreadyConnected);
            }
            let Some(saved_credential) = config.credentials.load()? else {
                return Ok(None);
            };
            let client_id = config
                .client_id
                .ok_or(AccountOperationError::ServiceUnavailable)?;
            let expected_account_id = saved_credential.expected_account_id().cloned();
            let client = build_client().map_err(|_| AccountOperationError::ServiceUnavailable)?;
            let response = identity::refresh(
                client_id,
                &config.endpoints,
                client.clone(),
                saved_credential.token().clone(),
            )
            .await?;

            let replacement_token = response
                .refresh_token()
                .cloned()
                .unwrap_or_else(|| saved_credential.token().clone());
            let pending_identity = CredentialIdentity::Unverified {
                expected_account_id: expected_account_id.clone(),
            };
            config
                .credentials
                .save(&pending_identity, &replacement_token)?;
            let access_token = Arc::new(response.access_token().clone());
            let profile = config
                .graph_api()
                .map_err(map_profile_graph_error)?
                .load_profile(&config.endpoints.graph_me, Arc::clone(&access_token))
                .await
                .map_err(map_profile_graph_error)?;
            if expected_account_id
                .as_ref()
                .is_some_and(|expected_account_id| expected_account_id != profile.id())
            {
                return Err(AccountOperationError::AccountMismatch);
            }
            config.credentials.save(
                &CredentialIdentity::Verified(profile.id().clone()),
                &replacement_token,
            )?;
            *session = Some(ProviderSession {
                account_id: profile.id().clone(),
                access_token: Some(access_token),
            });
            Ok(Some(profile))
        })
    }

    fn saved_credential_account_id(
        &self,
    ) -> impl Future<Output = Result<Option<AccountId>, AccountOperationError>> + Send + '_ {
        let config = Arc::clone(&self.config);
        worker::run(move |_cancellation: CancellationToken| async move {
            let session = config.session.lock().await;
            let Some(saved_credential) = config.credentials.load()? else {
                return Ok(session.as_ref().map(|session| session.account_id.clone()));
            };
            let Some(account_id) = saved_credential.account_id().cloned() else {
                return Ok(None);
            };
            if session
                .as_ref()
                .is_some_and(|session| session.account_id != account_id)
            {
                return Err(AccountOperationError::AccountMismatch);
            }
            Ok(Some(account_id))
        })
    }

    /// Connects a Microsoft 365 work account and stores its refresh token.
    ///
    /// # Errors
    ///
    /// Returns an authorization, browser, network, profile, or credential-store
    /// error when sign-in cannot complete. Returns
    /// [`AccountOperationError::AccountAlreadyConnected`] when this provider
    /// already has an active account session, or
    /// [`AccountOperationError::SavedAccountIdentityUnavailable`] when an
    /// unverified saved credential blocks replacement sign-in.
    fn connect_account(
        &self,
        cancellation: AccountOperationCancellation,
    ) -> impl Future<Output = Result<AccountProfile, AccountOperationError>> + Send + '_ {
        let config = Arc::clone(&self.config);
        let commit_cancellation = cancellation.clone();

        worker::run_with_account_cancellation(
            move |_cancellation: CancellationToken| async move {
                {
                    let session = config.session.lock().await;
                    if session.is_some() {
                        return Err(AccountOperationError::AccountAlreadyConnected);
                    }
                    ensure_saved_credential_can_be_replaced(&config, None)?;
                }
                let authorized =
                    authorize_profile(&config, AccountPermissionRequest::Current).await?;
                let mut session = config.session.lock().await;
                if session.is_some() {
                    return Err(AccountOperationError::AccountAlreadyConnected);
                }
                ensure_saved_credential_can_be_replaced(&config, Some(authorized.profile.id()))?;
                commit_cancellation.begin_credential_commit()?;
                config.credentials.save(
                    &CredentialIdentity::Verified(authorized.profile.id().clone()),
                    &authorized.refresh_token,
                )?;
                *session = Some(ProviderSession {
                    account_id: authorized.profile.id().clone(),
                    access_token: Some(authorized.access_token),
                });
                Ok(authorized.profile)
            },
            cancellation,
        )
    }

    fn reauthorize_account(
        &self,
        expected_account_id: &einfach_mail_domain::AccountId,
        request: AccountPermissionRequest,
        cancellation: AccountOperationCancellation,
    ) -> impl Future<Output = Result<AccountProfile, AccountOperationError>> + Send + '_ {
        let config = Arc::clone(&self.config);
        let expected_account_id = expected_account_id.clone();
        let commit_cancellation = cancellation.clone();

        worker::run_with_account_cancellation(
            move |_cancellation: CancellationToken| async move {
                let authorized = authorize_profile(&config, request).await?;
                if authorized.profile.id() != &expected_account_id {
                    return Err(AccountOperationError::AccountMismatch);
                }
                let mut session = config.session.lock().await;
                commit_cancellation.begin_credential_commit()?;
                config.credentials.save(
                    &CredentialIdentity::Verified(authorized.profile.id().clone()),
                    &authorized.refresh_token,
                )?;
                *session = Some(ProviderSession {
                    account_id: authorized.profile.id().clone(),
                    access_token: Some(authorized.access_token),
                });
                Ok(authorized.profile)
            },
            cancellation,
        )
    }

    /// Removes the saved refresh token from Linux Secret Service.
    ///
    /// This does not revoke consent at Microsoft.
    ///
    /// # Errors
    ///
    /// Returns a credential-store error when Secret Service cannot remove the
    /// token, [`AccountOperationError::SavedAccountIdentityUnavailable`] when
    /// the credential has no Graph-verified identity, or
    /// [`AccountOperationError::AccountMismatch`] when the saved identity does
    /// not match the active provider session.
    fn sign_out(&self) -> impl Future<Output = Result<(), AccountOperationError>> + Send + '_ {
        let config = Arc::clone(&self.config);
        worker::run(move |_cancellation: CancellationToken| async move {
            let mut session = config.session.lock().await;
            if let Some(saved_credential) = config.credentials.load()? {
                let Some(saved_account_id) = saved_credential.account_id() else {
                    return Err(AccountOperationError::SavedAccountIdentityUnavailable);
                };
                if session
                    .as_ref()
                    .is_some_and(|session| &session.account_id != saved_account_id)
                {
                    return Err(AccountOperationError::AccountMismatch);
                }
            }
            config.credentials.delete()?;
            *session = None;
            Ok(())
        })
    }
}

/// Checks that replacement keeps any verified saved account ID.
///
/// # Errors
///
/// Returns [`AccountOperationError::SavedAccountIdentityUnavailable`] when
/// the saved ID is unverified, or [`AccountOperationError::AccountMismatch`]
/// when the known profile has a different ID. Returns a credential-store
/// error when the saved credential cannot be read.
fn ensure_saved_credential_can_be_replaced(
    config: &ProviderConfig,
    account_id: Option<&AccountId>,
) -> Result<(), AccountOperationError> {
    let Some(saved_credential) = config.credentials.load()? else {
        return Ok(());
    };
    let Some(saved_account_id) = saved_credential.account_id() else {
        return Err(AccountOperationError::SavedAccountIdentityUnavailable);
    };
    if account_id.is_some_and(|account_id| account_id != saved_account_id) {
        return Err(AccountOperationError::AccountMismatch);
    }
    Ok(())
}

fn map_profile_graph_error(error: GraphError) -> AccountOperationError {
    match error {
        GraphError::AccessTokenRejected => AccountOperationError::AccessTokenRejected,
        GraphError::AccessDenied => AccountOperationError::AccessDenied,
        GraphError::InvalidResponse
        | GraphError::UnsafeLink
        | GraphError::InvalidRecord
        | GraphError::ResponseTooLarge
        | GraphError::PaginationLimitExceeded => AccountOperationError::ProfileUnavailable,
        GraphError::MessageUpdateOutcomeUnknown
        | GraphError::WorkerStart
        | GraphError::WorkerClosed
        | GraphError::Transport
        | GraphError::InvalidToken
        | GraphError::SendRejected
        | GraphError::SendOutcomeUnknown
        | GraphError::MessageMoveOutcomeUnknown
        | GraphError::CalendarEventCreationRejected
        | GraphError::CalendarEventCreationOutcomeUnknown
        | GraphError::CalendarEventUpdateOutcomeUnknown
        | GraphError::CalendarEventDeletionRejected
        | GraphError::CalendarEventDeletionOutcomeUnknown
        | GraphError::CalendarEventNotEditable
        | GraphError::CalendarEventChanged
        | GraphError::RateLimited { .. } => AccountOperationError::ServiceUnavailable,
    }
}

async fn authorize_profile(
    config: &ProviderConfig,
    request: AccountPermissionRequest,
) -> Result<AuthorizedAccount, AccountOperationError> {
    let client = build_client().map_err(|_| AccountOperationError::ServiceUnavailable)?;
    let response = identity::authorize(
        identity::AuthorizationContext {
            client_id: config
                .client_id
                .ok_or(AccountOperationError::ServiceUnavailable)?,
            callback_port: config.callback_port,
            endpoints: &config.endpoints,
            client,
            browser: config.browser.as_ref(),
        },
        request,
    )
    .await?;
    let refresh_token = response
        .refresh_token()
        .cloned()
        .ok_or(AccountOperationError::SignInFailed)?;
    let access_token = Arc::new(response.access_token().clone());
    let profile = config
        .graph_api()
        .map_err(map_profile_graph_error)?
        .load_profile(&config.endpoints.graph_me, Arc::clone(&access_token))
        .await
        .map_err(map_profile_graph_error)?;
    Ok(AuthorizedAccount {
        profile,
        refresh_token,
        access_token,
    })
}
