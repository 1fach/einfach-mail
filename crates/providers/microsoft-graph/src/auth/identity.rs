//! OAuth authorization-code and refresh-token operations.

use std::{
    net::{Ipv4Addr, SocketAddrV4},
    time::Duration,
};

use huskarl::{
    core::{EndpointUrl, Error as HuskarlError, RetryAdvice, client_auth::NoAuth},
    grant::{
        authorization_code::{AuthorizationCodeGrant, LoopbackError, StartInput, StartOutput},
        core::{OAuth2ExchangeGrant, TokenResponse},
        refresh::{RefreshGrant, RefreshGrantParameters},
    },
    token::RefreshToken,
};
use huskarl_reqwest::ReqwestClient;
use tokio::{net::TcpListener, time::timeout};
use url::Url;
use uuid::Uuid;

use crate::CallbackPort;
use einfach_mail_application::{AccountOperationError, AccountPermissionRequest};

const CALLBACK_PATH: &str = "/callback";
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(600);
const BASE_AUTHORIZATION_SCOPES: [&str; 4] =
    ["Calendars.Read", "Mail.Read", "User.Read", "offline_access"];
const CALENDAR_READ_WRITE_AUTHORIZATION_SCOPES: [&str; 4] = [
    "Calendars.ReadWrite",
    "Mail.Read",
    "User.Read",
    "offline_access",
];
pub const SEND_MAIL_AUTHORIZATION_SCOPES: [&str; 5] = [
    "Calendars.Read",
    "Mail.Read",
    "Mail.Send",
    "User.Read",
    "offline_access",
];
pub const MAIL_READ_WRITE_AUTHORIZATION_SCOPES: [&str; 4] = [
    "Calendars.Read",
    "Mail.ReadWrite",
    "User.Read",
    "offline_access",
];

#[derive(Clone)]
pub struct Endpoints {
    pub authorization: Url,
    pub token: Url,
    pub graph_me: Url,
}

impl Endpoints {
    pub fn microsoft() -> Self {
        Self {
            authorization: Url::parse(
                "https://login.microsoftonline.com/organizations/oauth2/v2.0/authorize",
            )
            .expect("the Microsoft authorization endpoint is a valid static URL"),
            token: Url::parse("https://login.microsoftonline.com/organizations/oauth2/v2.0/token")
                .expect("the Microsoft token endpoint is a valid static URL"),
            graph_me: Url::parse(
                "https://graph.microsoft.com/v1.0/me?$select=id,displayName,mail,userPrincipalName",
            )
            .expect("the Microsoft Graph profile endpoint is a valid static URL"),
        }
    }
}

pub trait BrowserLauncher: Send + Sync {
    fn open(&self, authorization_url: &Url) -> Result<(), AccountOperationError>;
}

pub struct AuthorizationContext<'context> {
    pub client_id: Uuid,
    pub callback_port: CallbackPort,
    pub endpoints: &'context Endpoints,
    pub client: reqwest::Client,
    pub browser: &'context dyn BrowserLauncher,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemBrowser;

impl BrowserLauncher for SystemBrowser {
    fn open(&self, authorization_url: &Url) -> Result<(), AccountOperationError> {
        webbrowser::open(authorization_url.as_str())
            .map_err(|_| AccountOperationError::ServiceUnavailable)
    }
}

pub async fn authorize(
    context: AuthorizationContext<'_>,
    request: AccountPermissionRequest,
) -> Result<TokenResponse, AccountOperationError> {
    let listener = bind_callback_listener(context.callback_port).await?;
    let redirect_uri = callback_uri(&listener)?;
    let grant = authorization_grant(
        context.client_id,
        &redirect_uri,
        context.endpoints,
        context.client.clone(),
    )
    .await?;
    let requested_scopes = authorization_scopes(request);
    let (started, authorization_url) =
        begin_authorization(&grant, &context, &redirect_uri, requested_scopes).await?;
    context.browser.open(&authorization_url)?;
    complete_authorization(&grant, &listener, &started).await
}

async fn bind_callback_listener(port: CallbackPort) -> Result<TcpListener, AccountOperationError> {
    TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port.number()))
        .await
        .map_err(|_| AccountOperationError::ServiceUnavailable)
}

fn callback_uri(listener: &TcpListener) -> Result<String, AccountOperationError> {
    let local_port = listener
        .local_addr()
        .map_err(|_| AccountOperationError::ServiceUnavailable)?
        .port();
    Ok(format!("http://127.0.0.1:{local_port}{CALLBACK_PATH}"))
}

async fn begin_authorization(
    grant: &AuthorizationCodeGrant,
    context: &AuthorizationContext<'_>,
    redirect_uri: &str,
    requested_scopes: &[&str],
) -> Result<(StartOutput, Url), AccountOperationError> {
    let started = grant
        .start(StartInput::scope(
            requested_scopes
                .iter()
                .map(|scope| (*scope).to_owned())
                .collect(),
        ))
        .await
        .map_err(|_| AccountOperationError::SignInFailed)?;
    let authorization_url = Url::parse(&started.authorization_url.to_string())
        .map_err(|_| AccountOperationError::SignInFailed)?;
    validate_authorization_request(
        &authorization_url,
        &AuthorizationExpectation {
            endpoint: &context.endpoints.authorization,
            client_id: context.client_id,
            redirect_uri,
            scopes: requested_scopes,
        },
    )?;
    Ok((started, authorization_url))
}

async fn complete_authorization(
    grant: &AuthorizationCodeGrant,
    listener: &TcpListener,
    started: &StartOutput,
) -> Result<TokenResponse, AccountOperationError> {
    let completed = timeout(
        CALLBACK_TIMEOUT,
        grant.complete_on_loopback(listener, &started.pending_state, None),
    )
    .await
    .map_err(|_| AccountOperationError::SignInFailed)?
    .map_err(map_loopback_error)?;
    Ok(completed.token_response)
}

pub async fn refresh(
    client_id: Uuid,
    endpoints: &Endpoints,
    client: reqwest::Client,
    refresh_token: RefreshToken,
) -> Result<TokenResponse, AccountOperationError> {
    let token_endpoint = endpoints
        .token
        .as_str()
        .parse::<EndpointUrl>()
        .map_err(|_| AccountOperationError::ServiceUnavailable)?;
    let grant = RefreshGrant::builder()
        .client_id(client_id.to_string())
        .http_client(ReqwestClient::from(client))
        .client_auth(NoAuth)
        .token_endpoint(token_endpoint)
        .build();

    grant
        .exchange(RefreshGrantParameters::refresh_token(refresh_token))
        .await
        .map_err(|error| map_refresh_error(&error))
}

async fn authorization_grant(
    client_id: Uuid,
    redirect_uri: &str,
    endpoints: &Endpoints,
    client: reqwest::Client,
) -> Result<AuthorizationCodeGrant, AccountOperationError> {
    let token_endpoint = endpoints
        .token
        .as_str()
        .parse::<EndpointUrl>()
        .map_err(|_| AccountOperationError::SignInFailed)?;
    let authorization_endpoint = endpoints
        .authorization
        .as_str()
        .parse::<EndpointUrl>()
        .map_err(|_| AccountOperationError::SignInFailed)?;
    AuthorizationCodeGrant::builder()
        .client_id(client_id.to_string())
        .client_auth(NoAuth)
        .http_client(ReqwestClient::from(client))
        .token_endpoint(token_endpoint)
        .authorization_endpoint(authorization_endpoint)
        .redirect_uri(redirect_uri)
        .oidc(false)
        .build()
        .await
        .map_err(|_| AccountOperationError::SignInFailed)
}

struct AuthorizationExpectation<'expected> {
    endpoint: &'expected Url,
    client_id: Uuid,
    redirect_uri: &'expected str,
    scopes: &'expected [&'expected str],
}

fn validate_authorization_request(
    url: &Url,
    expected: &AuthorizationExpectation<'_>,
) -> Result<(), AccountOperationError> {
    let endpoint_is_expected = url.origin() == expected.endpoint.origin()
        && url.path() == expected.endpoint.path()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none();
    let parameters = AuthorizationParameters::parse(url);

    if endpoint_is_expected && parameters.matches(expected) {
        Ok(())
    } else {
        Err(AccountOperationError::SignInFailed)
    }
}

struct AuthorizationParameters {
    response_type: Option<String>,
    client_id: Option<String>,
    state: Option<String>,
    redirect_uri: Option<String>,
    challenge_method: Option<String>,
    challenge: Option<String>,
    scopes: Option<Vec<String>>,
    includes_nonce: bool,
}

impl AuthorizationParameters {
    fn parse(url: &Url) -> Self {
        let raw_scopes = unique_query_value(url, "scope");
        Self {
            response_type: unique_query_value(url, "response_type"),
            client_id: unique_query_value(url, "client_id"),
            state: unique_query_value(url, "state"),
            redirect_uri: unique_query_value(url, "redirect_uri"),
            challenge_method: unique_query_value(url, "code_challenge_method"),
            challenge: unique_query_value(url, "code_challenge"),
            scopes: raw_scopes.map(|scope_string| {
                let mut scopes: Vec<_> = scope_string
                    .split_ascii_whitespace()
                    .map(str::to_owned)
                    .collect();
                scopes.sort_unstable();
                scopes
            }),
            includes_nonce: url
                .query_pairs()
                .any(|(query_name, _)| query_name == "nonce"),
        }
    }

    fn matches(&self, expected: &AuthorizationExpectation<'_>) -> bool {
        let expected_client_id = expected.client_id.to_string();
        self.response_type.as_deref() == Some("code")
            && self.client_id.as_deref() == Some(expected_client_id.as_str())
            && self.state.as_ref().is_some_and(|state| !state.is_empty())
            && self.redirect_uri.as_deref() == Some(expected.redirect_uri)
            && self.challenge_method.as_deref() == Some("S256")
            && self
                .challenge
                .as_ref()
                .is_some_and(|challenge| !challenge.is_empty())
            && self.has_exact_scopes(expected.scopes)
            && !self.includes_nonce
    }

    fn has_exact_scopes(&self, expected_scopes: &[&str]) -> bool {
        self.scopes.as_ref().is_some_and(|actual_scopes| {
            actual_scopes
                .iter()
                .map(String::as_str)
                .eq(expected_scopes.iter().copied())
        })
    }
}

pub fn authorization_scopes(request: AccountPermissionRequest) -> &'static [&'static str] {
    match request {
        AccountPermissionRequest::Current => &BASE_AUTHORIZATION_SCOPES,
        AccountPermissionRequest::MailReadWrite => &MAIL_READ_WRITE_AUTHORIZATION_SCOPES,
        AccountPermissionRequest::CalendarReadWrite => &CALENDAR_READ_WRITE_AUTHORIZATION_SCOPES,
        AccountPermissionRequest::SendMail => &SEND_MAIL_AUTHORIZATION_SCOPES,
    }
}

fn unique_query_value(url: &Url, expected_name: &str) -> Option<String> {
    let mut matching_values = url.query_pairs().filter_map(|(query_name, query_value)| {
        (query_name == expected_name).then(|| query_value.into_owned())
    });
    let only_value = matching_values.next()?;
    matching_values.next().is_none().then_some(only_value)
}

fn map_loopback_error(error: LoopbackError) -> AccountOperationError {
    match error {
        LoopbackError::OAuthError { verdict, .. } if verdict.code().as_str() == "access_denied" => {
            AccountOperationError::AccessDenied
        }
        LoopbackError::Complete { source } => map_authorization_error(&source),
        _ => AccountOperationError::SignInFailed,
    }
}

fn map_authorization_error(error: &HuskarlError) -> AccountOperationError {
    if error.verdict().is_some() || error.retry_advice() == RetryAdvice::No {
        AccountOperationError::SignInFailed
    } else {
        AccountOperationError::ServiceUnavailable
    }
}

fn map_refresh_error(error: &HuskarlError) -> AccountOperationError {
    if error
        .verdict()
        .is_some_and(|verdict| verdict.code().as_str() == "invalid_grant")
    {
        AccountOperationError::SignInExpired
    } else {
        AccountOperationError::ServiceUnavailable
    }
}

#[cfg(test)]
mod tests;
