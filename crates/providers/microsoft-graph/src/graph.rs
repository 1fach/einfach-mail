//! Microsoft Graph profile request and response mapping.

use huskarl::token::AccessToken;
use reqwest::{
    StatusCode,
    header::{AUTHORIZATION, HeaderValue},
};
use serde::Deserialize;
use url::Url;

use einfach_mail_application::AccountOperationError;
use einfach_mail_domain::{AccountId, AccountProfile};

const MAX_PROFILE_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphUserProfile {
    id: Option<String>,
    display_name: Option<String>,
    mail: Option<String>,
    user_principal_name: Option<String>,
}

pub(super) async fn load_profile(
    client: &reqwest::Client,
    endpoint: &Url,
    access_token: &AccessToken,
) -> Result<AccountProfile, AccountOperationError> {
    let authorization = authorization_header(access_token)?;
    let mut response = client
        .get(endpoint.clone())
        .header(AUTHORIZATION, authorization)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|_| AccountOperationError::ServiceUnavailable)?;

    match response.status() {
        status if status.is_success() => {}
        StatusCode::UNAUTHORIZED => return Err(AccountOperationError::AccessTokenRejected),
        StatusCode::FORBIDDEN => return Err(AccountOperationError::AccessDenied),
        _ => return Err(AccountOperationError::ServiceUnavailable),
    }

    if response
        .content_length()
        .is_some_and(|size| size > MAX_PROFILE_RESPONSE_BYTES as u64)
    {
        return Err(AccountOperationError::ProfileUnavailable);
    }

    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| AccountOperationError::ServiceUnavailable)?
    {
        if body.len().saturating_add(chunk.len()) > MAX_PROFILE_RESPONSE_BYTES {
            return Err(AccountOperationError::ProfileUnavailable);
        }
        body.extend_from_slice(&chunk);
    }

    let profile: GraphUserProfile =
        serde_json::from_slice(&body).map_err(|_| AccountOperationError::ProfileUnavailable)?;
    let id = profile
        .id
        .and_then(|id| AccountId::try_from(id).ok())
        .ok_or(AccountOperationError::ProfileUnavailable)?;

    AccountProfile::try_new(
        id,
        profile.display_name,
        profile.mail,
        profile.user_principal_name,
    )
    .map_err(|_| AccountOperationError::ProfileUnavailable)
}

fn authorization_header(access_token: &AccessToken) -> Result<HeaderValue, AccountOperationError> {
    let mut authorization = access_token
        .expose_header_value()
        .map_err(|_| AccountOperationError::SignInFailed)?;
    authorization.set_sensitive(true);
    Ok(authorization)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use huskarl::{
        core::secrets::SecretString,
        token::{AccessToken, BearerAccessToken},
    };

    use super::authorization_header;

    #[test]
    fn marks_the_graph_bearer_header_as_sensitive() {
        let access_token = AccessToken::Bearer(BearerAccessToken::new(
            SecretString::new("synthetic-access"),
            SystemTime::UNIX_EPOCH,
            Some(Duration::from_secs(3600)),
        ));

        let authorization =
            authorization_header(&access_token).expect("synthetic bearer token forms a header");

        assert!(authorization.is_sensitive());
    }
}
