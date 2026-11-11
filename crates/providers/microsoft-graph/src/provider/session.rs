use std::{sync::Arc, time::Duration};

use einfach_mail_application::AccountOperationError;
use einfach_mail_domain::AccountId;
use huskarl::token::AccessToken;
use reqwest::Client;

use crate::{
    auth::{
        credential::{CredentialIdentity, SavedRefreshToken},
        identity,
    },
    error::GraphError,
    transport::build_client,
};

use super::{ProviderConfig, ProviderSession};

const DEFAULT_ACCESS_TOKEN_LIFETIME: Duration = Duration::from_secs(3_600);
const ACCESS_TOKEN_EXPIRY_MARGIN: Duration = Duration::from_secs(300);

#[derive(Clone, Copy)]
pub(super) enum AccessTokenError {
    SignInRequired,
    AccountMismatch,
    AccessDenied,
    CredentialStoreUnavailable,
    ServiceUnavailable,
}

pub(super) async fn ensure_access_token(
    config: &ProviderConfig,
    account_id: &AccountId,
) -> Result<Arc<AccessToken>, AccessTokenError> {
    let mut session = config.session.lock().await;
    let Some(account_session) = session.as_ref() else {
        return Err(AccessTokenError::SignInRequired);
    };
    if &account_session.account_id != account_id {
        return Err(AccessTokenError::AccountMismatch);
    }
    if let Some(access_token) = account_session.access_token.as_ref()
        && !access_token.is_expired(DEFAULT_ACCESS_TOKEN_LIFETIME, ACCESS_TOKEN_EXPIRY_MARGIN)
    {
        return Ok(Arc::clone(access_token));
    }
    let saved_credential = config
        .credentials
        .load()
        .map_err(access_token_error_from_account)?
        .ok_or(AccessTokenError::SignInRequired)?;
    if saved_credential.account_id() != Some(account_id) {
        return Err(AccessTokenError::AccountMismatch);
    }
    let client = build_client().map_err(|_| AccessTokenError::ServiceUnavailable)?;
    refresh_access_token(config, &mut session, saved_credential, client).await
}

async fn refresh_access_token(
    config: &ProviderConfig,
    session: &mut Option<ProviderSession>,
    saved_credential: SavedRefreshToken,
    client: Client,
) -> Result<Arc<AccessToken>, AccessTokenError> {
    let response = identity::refresh(
        config
            .client_id
            .ok_or(AccessTokenError::ServiceUnavailable)?,
        &config.endpoints,
        client,
        saved_credential.token().clone(),
    )
    .await
    .map_err(access_token_error_from_account)?;
    let replacement_token = response
        .refresh_token()
        .cloned()
        .unwrap_or_else(|| saved_credential.token().clone());
    let access_token = Arc::new(response.access_token().clone());
    let Some(account_session) = session.as_mut() else {
        return Err(AccessTokenError::SignInRequired);
    };
    config
        .credentials
        .save(
            &CredentialIdentity::Verified(account_session.account_id.clone()),
            &replacement_token,
        )
        .map_err(access_token_error_from_account)?;
    account_session.access_token = Some(Arc::clone(&access_token));
    Ok(access_token)
}

fn access_token_error_from_account(error: AccountOperationError) -> AccessTokenError {
    match error {
        AccountOperationError::AccessDenied => AccessTokenError::AccessDenied,
        AccountOperationError::AccessTokenRejected | AccountOperationError::SignInExpired => {
            AccessTokenError::SignInRequired
        }
        AccountOperationError::CredentialStoreUnavailable => {
            AccessTokenError::CredentialStoreUnavailable
        }
        _ => AccessTokenError::ServiceUnavailable,
    }
}
pub(super) async fn invalidate_rejected_access_token<T>(
    config: &ProviderConfig,
    account_id: &AccountId,
    rejected_token: &Arc<AccessToken>,
    graph_result: Result<T, GraphError>,
) -> Result<T, GraphError> {
    if !matches!(&graph_result, Err(GraphError::AccessTokenRejected)) {
        return graph_result;
    }
    let mut session = config.session.lock().await;
    let Some(account_session) = session.as_mut() else {
        return graph_result;
    };
    if &account_session.account_id == account_id
        && account_session
            .access_token
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, rejected_token))
    {
        account_session.access_token = None;
    }
    graph_result
}
