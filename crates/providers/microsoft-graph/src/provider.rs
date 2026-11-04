//! Public Microsoft Graph provider and account workflow implementation.

use std::{
    future::Future,
    sync::{Arc, OnceLock},
    time::Duration,
};

use huskarl::token::{AccessToken, RefreshToken};
use reqwest::Client;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    CallbackPort,
    credential::{RefreshTokenStore, SecretServiceRefreshTokenStore},
    error::GraphError,
    graph::GraphApi,
    graph_url::GraphAuthority,
    http_client, identity,
    identity::{BrowserLauncher, Endpoints, SystemBrowser},
    transport::GraphTransport,
    worker,
};
use einfach_mail_application::{
    AccountOperationCancellation, AccountOperationError, AccountProvider,
    CalendarDeltaPageResponse, CalendarDeltaRequest, CalendarOperationError, CalendarProvider,
    MailOperationError, MailProvider, MessageDeltaRequest, MessageDeltaResponse,
};
use einfach_mail_domain::{
    AccountId, AccountProfile, CalendarViewReference, FolderReference, MailFolder, MailMessage,
    MessageBody, MessageId,
};

const DEFAULT_ACCESS_TOKEN_LIFETIME: Duration = Duration::from_secs(3_600);
const ACCESS_TOKEN_EXPIRY_MARGIN: Duration = Duration::from_secs(300);

/// Microsoft Graph account and mail provider for the Linux desktop application.
#[derive(Clone)]
pub struct MicrosoftGraphProvider {
    config: Arc<ProviderConfig>,
}

struct ProviderConfig {
    client_id: Uuid,
    callback_port: CallbackPort,
    endpoints: Endpoints,
    browser: Arc<dyn BrowserLauncher>,
    credentials: Arc<dyn RefreshTokenStore>,
    graph_authority: GraphAuthority,
    graph_transport: OnceLock<Result<GraphTransport, GraphError>>,
    session: tokio::sync::Mutex<Option<ProviderSession>>,
}

struct ProviderSession {
    account_id: AccountId,
    access_token: Option<Arc<AccessToken>>,
}

struct AuthorizedAccount {
    profile: AccountProfile,
    refresh_token: RefreshToken,
    access_token: Arc<AccessToken>,
}

impl MicrosoftGraphProvider {
    /// Creates a public-client provider for the `organizations` authority.
    ///
    /// The app ID must belong to a public client registration. The provider
    /// never accepts or stores a client secret.
    #[must_use]
    pub fn new(client_id: Uuid, callback_port: CallbackPort) -> Self {
        let endpoints = Endpoints::microsoft();
        let graph_authority = GraphAuthority::from_profile_endpoint(&endpoints.graph_me)
            .expect("the static Microsoft Graph profile URL is valid");
        Self {
            config: Arc::new(ProviderConfig {
                client_id,
                callback_port,
                endpoints,
                browser: Arc::new(SystemBrowser),
                credentials: Arc::new(SecretServiceRefreshTokenStore::new()),
                graph_authority,
                graph_transport: OnceLock::new(),
                session: tokio::sync::Mutex::new(None),
            }),
        }
    }
}

impl ProviderConfig {
    fn graph_api(&self) -> Result<GraphApi, GraphError> {
        let transport = self
            .graph_transport
            .get_or_init(GraphTransport::start)
            .as_ref()
            .map_err(|error| *error)?;
        Ok(GraphApi::new(
            self.graph_authority.clone(),
            transport.handle(),
        ))
    }
}

impl AccountProvider for MicrosoftGraphProvider {
    /// Renews the saved sign-in and loads the account profile.
    ///
    /// # Errors
    ///
    /// Returns a credential-store error when Secret Service cannot read or
    /// update the refresh token, an expired-sign-in error when Microsoft
    /// rejects it, or a service/profile error when refresh or Graph fails.
    fn restore_account(
        &self,
    ) -> impl Future<
        Output = Result<Option<einfach_mail_domain::AccountProfile>, AccountOperationError>,
    > + Send
    + '_ {
        let config = Arc::clone(&self.config);

        worker::run(move |_cancellation: CancellationToken| async move {
            let mut session = config.session.lock().await;
            let Some(refresh_token) = config.credentials.load()? else {
                *session = None;
                return Ok(None);
            };
            let client = http_client::build_client()
                .map_err(|_| AccountOperationError::ServiceUnavailable)?;
            let response = identity::refresh(
                config.client_id,
                &config.endpoints,
                client.clone(),
                refresh_token.clone(),
            )
            .await?;

            let replacement_token = response.refresh_token().cloned().unwrap_or(refresh_token);
            config.credentials.save(&replacement_token)?;
            let access_token = Arc::new(response.access_token().clone());
            let profile = config
                .graph_api()
                .map_err(map_profile_graph_error)?
                .load_profile(&config.endpoints.graph_me, Arc::clone(&access_token))
                .await
                .map_err(map_profile_graph_error)?;
            *session = Some(ProviderSession {
                account_id: profile.id().clone(),
                access_token: Some(access_token),
            });
            Ok(Some(profile))
        })
    }

    /// Connects a Microsoft 365 work account and stores its refresh token.
    ///
    /// # Errors
    ///
    /// Returns an authorization, browser, network, profile, or credential-store
    /// error when sign-in cannot complete.
    fn connect_account(
        &self,
        cancellation: AccountOperationCancellation,
    ) -> impl Future<Output = Result<AccountProfile, AccountOperationError>> + Send + '_ {
        let config = Arc::clone(&self.config);
        let commit_cancellation = cancellation.clone();

        worker::run_with_account_cancellation(
            move |_cancellation: CancellationToken| async move {
                let authorized = authorize_profile(&config).await?;
                let mut session = config.session.lock().await;
                commit_cancellation.begin_credential_commit()?;
                config.credentials.save(&authorized.refresh_token)?;
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
        cancellation: AccountOperationCancellation,
    ) -> impl Future<Output = Result<AccountProfile, AccountOperationError>> + Send + '_ {
        let config = Arc::clone(&self.config);
        let expected_account_id = expected_account_id.clone();
        let commit_cancellation = cancellation.clone();

        worker::run_with_account_cancellation(
            move |_cancellation: CancellationToken| async move {
                let authorized = authorize_profile(&config).await?;
                if authorized.profile.id() != &expected_account_id {
                    return Err(AccountOperationError::AccountMismatch);
                }
                let mut session = config.session.lock().await;
                commit_cancellation.begin_credential_commit()?;
                config.credentials.save(&authorized.refresh_token)?;
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
    /// token.
    fn sign_out(&self) -> impl Future<Output = Result<(), AccountOperationError>> + Send + '_ {
        let config = Arc::clone(&self.config);
        worker::run(move |_cancellation: CancellationToken| async move {
            let mut session = config.session.lock().await;
            config.credentials.delete()?;
            *session = None;
            Ok(())
        })
    }
}

impl MailProvider for MicrosoftGraphProvider {
    fn list_folders(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<Vec<MailFolder>, MailOperationError>> + Send + '_ {
        let config = Arc::clone(&self.config);
        let account_id = account_id.clone();
        worker::run(move |_cancellation: CancellationToken| async move {
            let token = ensure_access_token(&config, &account_id).await?;
            let graph_api = config.graph_api().map_err(map_graph_error)?;
            let folders = graph_api
                .list_folders(&account_id, Arc::clone(&token))
                .await;
            map_mail_result(&config, &account_id, &token, folders).await
        })
    }

    fn list_messages(
        &self,
        folder: &FolderReference,
    ) -> impl Future<Output = Result<Vec<MailMessage>, MailOperationError>> + Send + '_ {
        let config = Arc::clone(&self.config);
        let folder = folder.clone();
        worker::run(move |_cancellation: CancellationToken| async move {
            let account_id = folder.account_id();
            let token = ensure_access_token(&config, account_id).await?;
            let graph_api = config.graph_api().map_err(map_graph_error)?;
            let messages = graph_api.list_messages(&folder, Arc::clone(&token)).await;
            map_mail_result(&config, account_id, &token, messages).await
        })
    }

    fn message_delta_page(
        &self,
        folder: &FolderReference,
        request: MessageDeltaRequest,
    ) -> impl Future<Output = Result<MessageDeltaResponse, MailOperationError>> + Send + '_ {
        let config = Arc::clone(&self.config);
        let folder = folder.clone();
        worker::run(move |_cancellation: CancellationToken| async move {
            let account_id = folder.account_id();
            let token = ensure_access_token(&config, account_id).await?;
            let graph_api = config.graph_api().map_err(map_graph_error)?;
            let page = graph_api
                .message_delta_page(&folder, request, Arc::clone(&token))
                .await;
            map_mail_result(&config, account_id, &token, page).await
        })
    }

    fn fetch_message_body(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
    ) -> impl Future<Output = Result<MessageBody, MailOperationError>> + Send + '_ {
        let config = Arc::clone(&self.config);
        let account_id = folder.account_id().clone();
        let message_id = message_id.clone();
        worker::run(move |_cancellation: CancellationToken| async move {
            let token = ensure_access_token(&config, &account_id).await?;
            let graph_api = config.graph_api().map_err(map_graph_error)?;
            let body = graph_api
                .fetch_message_body(&message_id, Arc::clone(&token))
                .await;
            map_mail_result(&config, &account_id, &token, body).await
        })
    }
}

impl CalendarProvider for MicrosoftGraphProvider {
    fn calendar_delta_page(
        &self,
        view: &CalendarViewReference,
        request: CalendarDeltaRequest,
    ) -> impl Future<Output = Result<CalendarDeltaPageResponse, CalendarOperationError>> + Send + '_
    {
        let config = Arc::clone(&self.config);
        let view = view.clone();
        worker::run(move |_cancellation: CancellationToken| async move {
            let account_id = view.account_id().clone();
            let token = ensure_access_token(&config, &account_id)
                .await
                .map_err(map_mail_operation_to_calendar)?;
            let graph_api = config.graph_api().map_err(map_calendar_graph_error)?;
            let page = graph_api
                .calendar_delta_page(&view, request, Arc::clone(&token))
                .await;
            map_calendar_result(&config, &account_id, &token, page).await
        })
    }
}

async fn ensure_access_token(
    config: &ProviderConfig,
    account_id: &AccountId,
) -> Result<Arc<AccessToken>, MailOperationError> {
    let mut session = config.session.lock().await;
    let Some(account_session) = session.as_ref() else {
        return Err(MailOperationError::SignInRequired);
    };
    if &account_session.account_id != account_id {
        return Err(MailOperationError::AccountMismatch);
    }
    if let Some(access_token) = account_session.access_token.as_ref()
        && !access_token.is_expired(DEFAULT_ACCESS_TOKEN_LIFETIME, ACCESS_TOKEN_EXPIRY_MARGIN)
    {
        return Ok(Arc::clone(access_token));
    }
    let refresh_token = config
        .credentials
        .load()?
        .ok_or(MailOperationError::SignInRequired)?;
    let client = http_client::build_client().map_err(|_| MailOperationError::ServiceUnavailable)?;
    refresh_access_token(config, &mut session, refresh_token, client).await
}

async fn refresh_access_token(
    config: &ProviderConfig,
    session: &mut Option<ProviderSession>,
    refresh_token: RefreshToken,
    client: Client,
) -> Result<Arc<AccessToken>, MailOperationError> {
    let response = identity::refresh(
        config.client_id,
        &config.endpoints,
        client,
        refresh_token.clone(),
    )
    .await?;
    let replacement_token = response.refresh_token().cloned().unwrap_or(refresh_token);
    config.credentials.save(&replacement_token)?;
    let access_token = Arc::new(response.access_token().clone());
    let Some(account_session) = session.as_mut() else {
        return Err(MailOperationError::SignInRequired);
    };
    account_session.access_token = Some(Arc::clone(&access_token));
    Ok(access_token)
}

async fn map_mail_result<T>(
    config: &ProviderConfig,
    account_id: &AccountId,
    access_token: &Arc<AccessToken>,
    mail_result: Result<T, GraphError>,
) -> Result<T, MailOperationError> {
    if matches!(&mail_result, Err(GraphError::AccessTokenRejected)) {
        invalidate_access_token(config, account_id, access_token).await;
    }
    mail_result.map_err(map_graph_error)
}

async fn map_calendar_result<T>(
    config: &ProviderConfig,
    account_id: &AccountId,
    access_token: &Arc<AccessToken>,
    calendar_result: Result<T, GraphError>,
) -> Result<T, CalendarOperationError> {
    if matches!(&calendar_result, Err(GraphError::AccessTokenRejected)) {
        invalidate_access_token(config, account_id, access_token).await;
    }
    calendar_result.map_err(map_calendar_graph_error)
}

fn map_mail_operation_to_calendar(error: MailOperationError) -> CalendarOperationError {
    match error {
        MailOperationError::SignInRequired => CalendarOperationError::SignInRequired,
        MailOperationError::AccountMismatch => CalendarOperationError::AccountMismatch,
        MailOperationError::AccessDenied => CalendarOperationError::AccessDenied,
        MailOperationError::RateLimited { retry_after } => {
            CalendarOperationError::RateLimited { retry_after }
        }
        MailOperationError::InvalidResponse => CalendarOperationError::InvalidResponse,
        MailOperationError::ResourceLimitExceeded => CalendarOperationError::ResourceLimitExceeded,
        MailOperationError::CredentialStoreUnavailable | MailOperationError::ServiceUnavailable => {
            CalendarOperationError::ServiceUnavailable
        }
        _ => CalendarOperationError::ServiceUnavailable,
    }
}

fn map_calendar_graph_error(error: GraphError) -> CalendarOperationError {
    match error {
        GraphError::AccessTokenRejected => CalendarOperationError::SignInRequired,
        GraphError::AccessDenied => CalendarOperationError::AccessDenied,
        GraphError::RateLimited { retry_after } => {
            CalendarOperationError::RateLimited { retry_after }
        }
        GraphError::ResponseTooLarge | GraphError::PaginationLimitExceeded => {
            CalendarOperationError::ResourceLimitExceeded
        }
        GraphError::InvalidResponse | GraphError::UnsafeLink | GraphError::InvalidRecord => {
            CalendarOperationError::InvalidResponse
        }
        GraphError::WorkerStart
        | GraphError::WorkerClosed
        | GraphError::Transport
        | GraphError::InvalidToken => CalendarOperationError::ServiceUnavailable,
    }
}

async fn invalidate_access_token(
    config: &ProviderConfig,
    account_id: &AccountId,
    rejected_token: &Arc<AccessToken>,
) {
    let mut session = config.session.lock().await;
    let Some(account_session) = session.as_mut() else {
        return;
    };
    if &account_session.account_id == account_id
        && account_session
            .access_token
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, rejected_token))
    {
        account_session.access_token = None;
    }
}

fn map_graph_error(error: GraphError) -> MailOperationError {
    match error {
        GraphError::AccessTokenRejected => MailOperationError::SignInRequired,
        GraphError::AccessDenied => MailOperationError::AccessDenied,
        GraphError::RateLimited { retry_after } => MailOperationError::RateLimited { retry_after },
        GraphError::ResponseTooLarge | GraphError::PaginationLimitExceeded => {
            MailOperationError::ResourceLimitExceeded
        }
        GraphError::InvalidResponse | GraphError::UnsafeLink | GraphError::InvalidRecord => {
            MailOperationError::InvalidResponse
        }
        GraphError::WorkerStart
        | GraphError::WorkerClosed
        | GraphError::Transport
        | GraphError::InvalidToken => MailOperationError::ServiceUnavailable,
    }
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
        GraphError::WorkerStart
        | GraphError::WorkerClosed
        | GraphError::Transport
        | GraphError::InvalidToken
        | GraphError::RateLimited { .. } => AccountOperationError::ServiceUnavailable,
    }
}

async fn authorize_profile(
    config: &ProviderConfig,
) -> Result<AuthorizedAccount, AccountOperationError> {
    let client =
        http_client::build_client().map_err(|_| AccountOperationError::ServiceUnavailable)?;
    let response = identity::authorize(
        config.client_id,
        config.callback_port,
        &config.endpoints,
        client.clone(),
        config.browser.as_ref(),
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

#[cfg(test)]
mod tests {
    use std::{
        net::{Ipv4Addr, SocketAddrV4},
        sync::{Arc, Barrier, Mutex, OnceLock},
        time::Duration,
    };

    use httpmock::{
        Method::{GET, POST},
        MockServer,
    };
    use huskarl::token::RefreshToken;
    use tokio::net::TcpListener;
    use tokio::sync::{mpsc, oneshot};
    use url::Url;

    use crate::{
        CallbackPort,
        credential::RefreshTokenStore,
        error::GraphError,
        graph_url::GraphAuthority,
        identity::{BrowserLauncher, Endpoints},
        transport::GraphTransport,
    };
    use einfach_mail_application::{
        AccountOperationCancellation, AccountOperationError, AccountProvider, AccountWorkflow,
        MailOperationError, MailProvider, MessageDeltaRequest, MessageDeltaResponse,
    };
    use einfach_mail_domain::{
        AccountId, FolderReference, MessageBodyType, MessageDeltaLink, MessageDeltaProgress,
    };

    use super::{MicrosoftGraphProvider, ProviderConfig};

    struct MemoryRefreshTokenStore {
        token: Mutex<Option<RefreshToken>>,
        save_pause: Mutex<Option<(Arc<Barrier>, oneshot::Sender<()>)>>,
    }

    impl MemoryRefreshTokenStore {
        fn new(token: Option<RefreshToken>) -> Self {
            Self {
                token: Mutex::new(token),
                save_pause: Mutex::new(None),
            }
        }

        fn pause_next_save_after_write(&self) -> (oneshot::Receiver<()>, Arc<Barrier>) {
            let barrier = Arc::new(Barrier::new(2));
            let (started_sender, started_receiver) = oneshot::channel();
            *self
                .save_pause
                .lock()
                .expect("synthetic credential mutex is not poisoned") =
                Some((Arc::clone(&barrier), started_sender));
            (started_receiver, barrier)
        }

        fn token_value(&self) -> Option<String> {
            self.token
                .lock()
                .expect("synthetic credential mutex is not poisoned")
                .as_ref()
                .map(|token| token.token().expose_secret().to_owned())
        }
    }

    impl RefreshTokenStore for MemoryRefreshTokenStore {
        fn load(&self) -> Result<Option<RefreshToken>, AccountOperationError> {
            self.token
                .lock()
                .map(|token| token.clone())
                .map_err(|_| AccountOperationError::CredentialStoreUnavailable)
        }

        fn save(&self, token: &RefreshToken) -> Result<(), AccountOperationError> {
            {
                let mut saved = self
                    .token
                    .lock()
                    .map_err(|_| AccountOperationError::CredentialStoreUnavailable)?;
                *saved = Some(token.clone());
            }
            let save_pause = self
                .save_pause
                .lock()
                .map_err(|_| AccountOperationError::CredentialStoreUnavailable)?
                .take();
            if let Some((barrier, started_sender)) = save_pause {
                let _ = started_sender.send(());
                barrier.wait();
            }
            Ok(())
        }

        fn delete(&self) -> Result<(), AccountOperationError> {
            self.token
                .lock()
                .map(|mut saved| *saved = None)
                .map_err(|_| AccountOperationError::CredentialStoreUnavailable)
        }
    }

    struct TestBrowser {
        sender: mpsc::UnboundedSender<Url>,
    }

    impl BrowserLauncher for TestBrowser {
        fn open(&self, authorization_url: &Url) -> Result<(), AccountOperationError> {
            self.sender
                .send(authorization_url.clone())
                .map_err(|_| AccountOperationError::ServiceUnavailable)
        }
    }

    fn refresh_token(token_text: &str) -> RefreshToken {
        serde_json::from_str(&format!(r#"{{"token":"{token_text}"}}"#))
            .expect("synthetic refresh token is valid")
    }

    fn synthetic_provider(
        server: &MockServer,
        sender: mpsc::UnboundedSender<Url>,
        credentials: Arc<MemoryRefreshTokenStore>,
    ) -> MicrosoftGraphProvider {
        let endpoints = Endpoints {
            authorization: Url::parse(&server.url("/authorize"))
                .expect("synthetic authorization endpoint is valid"),
            token: Url::parse(&server.url("/token")).expect("synthetic token endpoint is valid"),
            graph_me: Url::parse(&format!(
                "{}?$select=id,displayName,mail,userPrincipalName",
                server.url("/v1.0/me")
            ))
            .expect("synthetic Graph endpoint is valid"),
        };
        let config = ProviderConfig {
            client_id: uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000001")
                .expect("synthetic client ID is valid"),
            callback_port: CallbackPort::Ephemeral,
            graph_authority: GraphAuthority::from_profile_endpoint(&endpoints.graph_me)
                .expect("synthetic Graph authority is valid"),
            endpoints,
            browser: Arc::new(TestBrowser { sender }),
            credentials,
            graph_transport: fixture_transport(),
            session: tokio::sync::Mutex::new(None),
        };

        MicrosoftGraphProvider {
            config: Arc::new(config),
        }
    }

    fn fixture_transport() -> OnceLock<Result<GraphTransport, GraphError>> {
        let transport = GraphTransport::start_for_fixtures().expect("fixture transport starts");
        let cell = OnceLock::new();
        assert!(cell.set(Ok(transport)).is_ok());
        cell
    }

    fn has_scope(parameters: &std::collections::HashMap<String, String>, expected: &str) -> bool {
        parameters.get("scope").is_some_and(|scope| {
            scope
                .split_ascii_whitespace()
                .any(|scope_name| scope_name == expected)
        })
    }

    fn callback_url_for_response(
        authorization_url: &Url,
        response_parameter: &str,
        response_value: &str,
    ) -> Url {
        let redirect_uri = authorization_url
            .query_pairs()
            .find_map(|(name, value)| (name == "redirect_uri").then(|| value.into_owned()))
            .expect("authorization request has a redirect URI");
        let oauth_state = authorization_url
            .query_pairs()
            .find_map(|(name, value)| (name == "state").then(|| value.into_owned()))
            .expect("authorization request has state");
        let mut callback_url = Url::parse(&redirect_uri).expect("callback URI is valid");
        callback_url
            .query_pairs_mut()
            .append_pair(response_parameter, response_value)
            .append_pair("state", &oauth_state);
        callback_url
    }

    async fn complete_test_authorization(
        browser_receiver: &mut mpsc::UnboundedReceiver<Url>,
        code: &str,
    ) {
        let authorization_url = browser_receiver
            .recv()
            .await
            .expect("synthetic authorization opens the test browser");
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("test HTTP client is valid")
            .get(callback_url_for_response(&authorization_url, "code", code))
            .send()
            .await
            .expect("loopback callback reaches the test listener");
        assert!(response.status().is_success());
    }

    #[test]
    fn callback_port_defaults_to_ephemeral_and_rejects_invalid_values() {
        assert_eq!(
            CallbackPort::parse_environment_value(None),
            Ok(CallbackPort::Ephemeral)
        );
        assert_eq!(
            CallbackPort::parse_environment_value(Some("0")),
            Ok(CallbackPort::Ephemeral)
        );
        assert_eq!(
            CallbackPort::parse_environment_value(Some("4187")),
            Ok(CallbackPort::Fixed(
                std::num::NonZeroU16::new(4187).expect("non-zero port")
            ))
        );
        assert!(CallbackPort::parse_environment_value(Some("70000")).is_err());
        assert!(CallbackPort::parse_environment_value(Some("invalid")).is_err());
    }

    #[tokio::test]
    async fn connect_runs_pkce_callback_graph_profile_and_token_storage() {
        let server = MockServer::start_async().await;
        let token_exchange = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/token")
                    .body_includes("grant_type=authorization_code")
                    .body_includes("code_verifier=");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(
                        r#"{"token_type":"Bearer","access_token":"synthetic-access","expires_in":3600,"refresh_token":"synthetic-refresh","scope":"Calendars.Read Mail.Read User.Read offline_access"}"#,
                    );
            })
            .await;
        let profile_request = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/v1.0/me")
                    .query_param("$select", "id,displayName,mail,userPrincipalName")
                    .header("authorization", "Bearer synthetic-access");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(
                        r#"{"id":"account-123","displayName":"Ada Example","mail":"ada@example.test","userPrincipalName":"ada@tenant.example.test"}"#,
                    );
            })
            .await;
        let folder_request = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/v1.0/me/mailFolders")
                    .header("authorization", "Bearer synthetic-access");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(r#"{"value":[]}"#);
            })
            .await;
        let (browser_sender, mut browser_receiver) = mpsc::unbounded_channel();
        let credentials = Arc::new(MemoryRefreshTokenStore::new(None));
        let provider = synthetic_provider(&server, browser_sender, credentials.clone());
        let connect_provider = provider.clone();
        let connect_task = tokio::spawn(async move {
            connect_provider
                .connect_account(AccountOperationCancellation::new())
                .await
        });
        let authorization_url = browser_receiver
            .recv()
            .await
            .expect("sign-in opens the test browser");

        let parameters = authorization_url
            .query_pairs()
            .map(|(query_name, query_value)| (query_name.into_owned(), query_value.into_owned()))
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(
            parameters.get("response_type").map(String::as_str),
            Some("code")
        );
        assert!(has_scope(&parameters, "User.Read"));
        assert!(has_scope(&parameters, "Mail.Read"));
        assert!(has_scope(&parameters, "Calendars.Read"));
        assert!(has_scope(&parameters, "offline_access"));
        assert_eq!(
            parameters.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert!(
            parameters
                .get("code_challenge")
                .is_some_and(|challenge| !challenge.is_empty())
        );
        assert!(!parameters.contains_key("nonce"));

        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("test HTTP client is valid")
            .get(callback_url_for_response(
                &authorization_url,
                "code",
                "synthetic-authorization-code",
            ))
            .send()
            .await
            .expect("loopback callback reaches the test listener");
        assert!(response.status().is_success());

        let profile = connect_task
            .await
            .expect("provider task completes")
            .expect("synthetic account connection succeeds");
        assert_eq!(profile.id().as_str(), "account-123");
        assert_eq!(profile.label(), "Ada Example");
        assert_eq!(
            profile.mailbox().map(|mailbox| mailbox.address()),
            Some("ada@example.test")
        );
        assert_eq!(
            profile.user_principal_name(),
            Some("ada@tenant.example.test")
        );
        assert_eq!(
            credentials.token_value().as_deref(),
            Some("synthetic-refresh")
        );
        assert!(
            provider
                .list_folders(profile.id())
                .await
                .expect("connected session loads mail folders")
                .is_empty()
        );
        token_exchange.assert_async().await;
        profile_request.assert_async().await;
        folder_request.assert_async().await;

        provider
            .sign_out()
            .await
            .expect("local sign-out removes the synthetic credential");
        assert_eq!(credentials.token_value(), None);
    }

    #[tokio::test]
    async fn mail_provider_reads_scoped_folders_messages_delta_and_body() {
        let server = MockServer::start_async().await;
        let token_refresh = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/token")
                    .body_includes("grant_type=refresh_token")
                    .body_includes("refresh_token=synthetic-old-refresh");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(
                        r#"{"token_type":"Bearer","access_token":"synthetic-mail-access","expires_in":3600,"refresh_token":"synthetic-rotated-refresh","scope":"Mail.Read User.Read offline_access"}"#,
                    );
            })
            .await;
        let profile_request = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/v1.0/me")
                    .header("authorization", "Bearer synthetic-mail-access");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(r#"{"id":"account-mail","displayName":"Synthetic Account"}"#);
            })
            .await;
        let root_folders = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/v1.0/me/mailFolders")
                    .header("authorization", "Bearer synthetic-mail-access");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(
                        r#"{"value":[{"id":"folder-inbox","displayName":"Inbox","isHidden":false}]}"#,
                    );
            })
            .await;
        let child_folders = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/v1.0/me/mailFolders/folder-inbox/childFolders")
                    .header("authorization", "Bearer synthetic-mail-access");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(r#"{"value":[]}"#);
            })
            .await;
        let message_list = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/v1.0/me/mailFolders/folder-inbox/messages")
                    .header("authorization", "Bearer synthetic-mail-access")
                    .header("prefer", "IdType=\"ImmutableId\"");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(
                        r#"{"value":[{"id":"message-1","subject":"Synthetic message","receivedDateTime":"2026-10-31T12:00:00Z","isRead":false}]}"#,
                    );
            })
            .await;
        let next_link = format!(
            "{}/v1.0/me/mailFolders/folder-inbox/messages/delta?$skiptoken=opaque-next",
            server.base_url()
        );
        let delta_link = format!(
            "{}/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken=opaque-final",
            server.base_url()
        );
        let message_delta_initial = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/v1.0/me/mailFolders/folder-inbox/messages/delta")
                    .query_param_missing("$skiptoken")
                    .header("authorization", "Bearer synthetic-mail-access")
                    .header("content-type", "application/json");
                then.status(200)
                    .header("content-type", "application/json")
                    .json_body(serde_json::json!({
                        "value": [],
                        "@odata.nextLink": next_link,
                    }));
            })
            .await;
        let message_delta_continuation = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/v1.0/me/mailFolders/folder-inbox/messages/delta")
                    .query_param("$skiptoken", "opaque-next")
                    .header("authorization", "Bearer synthetic-mail-access")
                    .header("content-type", "application/json");
                then.status(200)
                    .header("content-type", "application/json")
                    .json_body(serde_json::json!({
                        "value": [],
                        "@odata.deltaLink": delta_link,
                    }));
            })
            .await;
        let message_body = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/v1.0/me/messages/message-1")
                    .query_param("$select", "body")
                    .header("authorization", "Bearer synthetic-mail-access");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(r#"{"body":{"contentType":"html","content":"<p>Safe fixture</p>"}}"#);
            })
            .await;
        let (browser_sender, _browser_receiver) = mpsc::unbounded_channel();
        let credentials = Arc::new(MemoryRefreshTokenStore::new(Some(refresh_token(
            "synthetic-old-refresh",
        ))));
        let provider = synthetic_provider(&server, browser_sender, credentials.clone());

        let profile = provider
            .restore_account()
            .await
            .expect("synthetic account restores")
            .expect("synthetic account is present");
        let folders = provider
            .list_folders(profile.id())
            .await
            .expect("provider lists synthetic folders");
        let folder = folders
            .first()
            .expect("synthetic inbox is present")
            .reference()
            .clone();
        let messages = provider
            .list_messages(&folder)
            .await
            .expect("provider lists synthetic message metadata");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].folder(), &folder);

        let response = provider
            .message_delta_page(&folder, MessageDeltaRequest::Initial)
            .await
            .expect("provider loads one synthetic delta page");
        let MessageDeltaResponse::Page(page) = response else {
            panic!("synthetic provider response is a page");
        };
        assert!(page.response_bytes() > 0);
        let (page, _) = page.into_parts();
        let MessageDeltaProgress::Next(next_link) = page.progress() else {
            panic!("synthetic initial response contains a continuation link");
        };
        let response = provider
            .message_delta_page(
                &folder,
                MessageDeltaRequest::Continue(MessageDeltaLink::from_opaque(
                    next_link.as_str().to_owned(),
                )),
            )
            .await
            .expect("provider follows one synthetic continuation link");
        let MessageDeltaResponse::Page(page) = response else {
            panic!("synthetic continuation response is a page");
        };
        assert!(matches!(
            page.into_parts().0.progress(),
            MessageDeltaProgress::Complete(_)
        ));
        let body = provider
            .fetch_message_body(&folder, messages[0].id())
            .await
            .expect("provider loads the requested synthetic body");
        assert_eq!(body.content_type(), MessageBodyType::Html);
        assert_eq!(body.content(), "<p>Safe fixture</p>");

        let another_account = AccountId::try_from("another-account".to_owned())
            .expect("synthetic account ID is non-empty");
        let another_folder = FolderReference::new(another_account, folder.folder_id().clone());
        assert_eq!(
            provider.list_messages(&another_folder).await,
            Err(MailOperationError::AccountMismatch)
        );

        token_refresh.assert_async().await;
        profile_request.assert_async().await;
        root_folders.assert_async().await;
        child_folders.assert_async().await;
        message_list.assert_async().await;
        message_delta_initial.assert_async().await;
        message_delta_continuation.assert_async().await;
        message_body.assert_async().await;
        assert_eq!(
            credentials.token_value().as_deref(),
            Some("synthetic-rotated-refresh")
        );
    }

    #[tokio::test]
    async fn denied_consent_returns_a_user_safe_error_and_cleans_up_callback() {
        let server = MockServer::start_async().await;
        let (browser_sender, mut browser_receiver) = mpsc::unbounded_channel();
        let credentials = Arc::new(MemoryRefreshTokenStore::new(None));
        let provider = synthetic_provider(&server, browser_sender, credentials.clone());
        let connect_provider = provider.clone();
        let connect_task = tokio::spawn(async move {
            connect_provider
                .connect_account(AccountOperationCancellation::new())
                .await
        });
        let authorization_url = browser_receiver
            .recv()
            .await
            .expect("sign-in opens the synthetic browser");
        let callback_response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("synthetic callback client is valid")
            .get(callback_url_for_response(
                &authorization_url,
                "error",
                "access_denied",
            ))
            .send()
            .await
            .expect("denied callback reaches the listener");

        assert!(callback_response.status().is_success());
        assert_eq!(
            connect_task.await.expect("provider task completes"),
            Err(AccountOperationError::AccessDenied)
        );
        assert_eq!(credentials.token_value(), None);
    }

    #[tokio::test]
    async fn cancelling_sign_in_releases_the_loopback_callback_listener() {
        let server = MockServer::start_async().await;
        let (browser_sender, mut browser_receiver) = mpsc::unbounded_channel();
        let credentials = Arc::new(MemoryRefreshTokenStore::new(Some(refresh_token(
            "synthetic-existing-refresh",
        ))));
        let provider = synthetic_provider(&server, browser_sender, credentials.clone());
        let cancellation = AccountOperationCancellation::new();
        let task_cancellation = cancellation.clone();
        let connect_provider = provider.clone();
        let connect_task =
            tokio::spawn(async move { connect_provider.connect_account(task_cancellation).await });
        let authorization_url = browser_receiver
            .recv()
            .await
            .expect("sign-in opens the synthetic browser");
        let callback_port = Url::parse(
            &authorization_url
                .query_pairs()
                .find_map(|(name, value)| (name == "redirect_uri").then(|| value.into_owned()))
                .expect("authorization request has a redirect URI"),
        )
        .expect("callback URI is valid")
        .port()
        .expect("callback URI has a port");

        cancellation.cancel();
        assert_eq!(
            connect_task.await.expect("cancelled sign-in completes"),
            Err(AccountOperationError::SignInCancelled)
        );
        assert_eq!(
            credentials.token_value().as_deref(),
            Some("synthetic-existing-refresh")
        );
        let callback_listener = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Ok(listener) =
                    TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, callback_port)).await
                {
                    break listener;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("cancellation releases the callback port");

        assert!(callback_listener.local_addr().is_ok());
        assert_eq!(
            credentials.token_value().as_deref(),
            Some("synthetic-existing-refresh")
        );
    }

    #[tokio::test]
    async fn restore_persists_rotated_refresh_token_and_loads_profile() {
        let server = MockServer::start_async().await;
        let token_refresh = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/token")
                    .body_includes("grant_type=refresh_token")
                    .body_includes("refresh_token=synthetic-old-refresh");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(
                        r#"{"token_type":"Bearer","access_token":"synthetic-renewed-access","expires_in":3600,"refresh_token":"synthetic-rotated-refresh","scope":"Calendars.Read Mail.Read User.Read offline_access"}"#,
                    );
            })
            .await;
        let profile_request = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/v1.0/me")
                    .header("authorization", "Bearer synthetic-renewed-access");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(
                        r#"{"id":"account-456","displayName":"Grace Example","mail":"grace@example.test","userPrincipalName":"grace@tenant.example.test"}"#,
                    );
            })
            .await;
        let (browser_sender, _browser_receiver) = mpsc::unbounded_channel();
        let credentials = Arc::new(MemoryRefreshTokenStore::new(Some(refresh_token(
            "synthetic-old-refresh",
        ))));
        let provider = synthetic_provider(&server, browser_sender, credentials.clone());

        let restored = provider
            .restore_account()
            .await
            .expect("saved synthetic account is restored");
        let profile = restored.expect("saved refresh token identifies an account");

        assert_eq!(profile.label(), "Grace Example");
        assert_eq!(
            credentials.token_value().as_deref(),
            Some("synthetic-rotated-refresh")
        );
        token_refresh.assert_async().await;
        profile_request.assert_async().await;
    }

    #[tokio::test]
    async fn restore_reports_a_rejected_graph_access_token() {
        let server = MockServer::start_async().await;
        let token_refresh = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/token")
                    .body_includes("grant_type=refresh_token");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(
                        r#"{"token_type":"Bearer","access_token":"synthetic-rejected-access","expires_in":3600,"refresh_token":"synthetic-rotated-refresh","scope":"Calendars.Read Mail.Read User.Read offline_access"}"#,
                    );
            })
            .await;
        let profile_request = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/v1.0/me")
                    .header("authorization", "Bearer synthetic-rejected-access");
                then.status(401);
            })
            .await;
        let (browser_sender, _browser_receiver) = mpsc::unbounded_channel();
        let credentials = Arc::new(MemoryRefreshTokenStore::new(Some(refresh_token(
            "synthetic-old-refresh",
        ))));
        let provider = synthetic_provider(&server, browser_sender, credentials.clone());

        assert_eq!(
            provider.restore_account().await,
            Err(AccountOperationError::AccessTokenRejected)
        );
        assert_eq!(
            credentials.token_value().as_deref(),
            Some("synthetic-rotated-refresh")
        );
        token_refresh.assert_async().await;
        profile_request.assert_async().await;
    }

    #[tokio::test]
    async fn restore_maps_invalid_grant_to_an_expired_sign_in() {
        let server = MockServer::start_async().await;
        let token_refresh = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/token")
                    .body_includes("grant_type=refresh_token");
                then.status(400)
                    .header("content-type", "application/json")
                    .body(r#"{"error":"invalid_grant"}"#);
            })
            .await;
        let (browser_sender, _browser_receiver) = mpsc::unbounded_channel();
        let credentials = Arc::new(MemoryRefreshTokenStore::new(Some(refresh_token(
            "synthetic-rejected-refresh",
        ))));
        let provider = synthetic_provider(&server, browser_sender, credentials.clone());

        assert_eq!(
            provider.restore_account().await,
            Err(AccountOperationError::SignInExpired)
        );
        assert_eq!(
            credentials.token_value().as_deref(),
            Some("synthetic-rejected-refresh")
        );
        token_refresh.assert_async().await;
    }

    #[tokio::test]
    async fn connection_cancellation_during_commit_returns_and_restores_the_profile() {
        let server = MockServer::start_async().await;
        let token_exchange = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/token")
                    .body_includes("grant_type=authorization_code");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(
                        r#"{"token_type":"Bearer","access_token":"synthetic-connect-access","expires_in":3600,"refresh_token":"synthetic-connect-refresh","scope":"Calendars.Read Mail.Read User.Read offline_access"}"#,
                    );
            })
            .await;
        let profile_request = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/v1.0/me")
                    .header("authorization", "Bearer synthetic-connect-access");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(r#"{"id":"account-123","displayName":"Ada Example"}"#);
            })
            .await;
        let restored_token = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/token")
                    .body_includes("grant_type=refresh_token")
                    .body_includes("refresh_token=synthetic-connect-refresh");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(
                        r#"{"token_type":"Bearer","access_token":"synthetic-restored-access","expires_in":3600,"refresh_token":"synthetic-recovered-refresh","scope":"User.Read offline_access"}"#,
                    );
            })
            .await;
        let restored_profile_request = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/v1.0/me")
                    .header("authorization", "Bearer synthetic-restored-access");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(r#"{"id":"account-123","displayName":"Ada Example"}"#);
            })
            .await;
        let (browser_sender, mut browser_receiver) = mpsc::unbounded_channel();
        let credentials = Arc::new(MemoryRefreshTokenStore::new(None));
        let (save_started, finish_save) = credentials.pause_next_save_after_write();
        let provider = synthetic_provider(&server, browser_sender, credentials.clone());
        let workflow = Arc::new(AccountWorkflow::new(provider));
        let cancellation = AccountOperationCancellation::new();
        let task_cancellation = cancellation.clone();
        let task_workflow = Arc::clone(&workflow);
        let connection = tokio::spawn(async move {
            task_workflow
                .connect_account_with_cancellation(task_cancellation)
                .await
        });

        complete_test_authorization(&mut browser_receiver, "synthetic-connect-code").await;
        tokio::time::timeout(Duration::from_secs(2), save_started)
            .await
            .expect("credential commit begins")
            .expect("synthetic store signals its pending write");
        cancellation.cancel();
        finish_save.wait();

        let connected_profile = connection
            .await
            .expect("connection workflow completes")
            .expect("commit wins the cancellation race");
        assert_eq!(connected_profile.id().as_str(), "account-123");
        assert_eq!(
            credentials.token_value().as_deref(),
            Some("synthetic-connect-refresh")
        );

        let (restored_browser_sender, _) = mpsc::unbounded_channel();
        let restored_provider =
            synthetic_provider(&server, restored_browser_sender, credentials.clone());
        let restored_workflow = AccountWorkflow::new(restored_provider);
        let restored_profile = restored_workflow
            .restore_account()
            .await
            .expect("new workflow restores the committed account")
            .expect("committed account is present");
        assert_eq!(restored_profile.id(), connected_profile.id());
        assert_eq!(
            credentials.token_value().as_deref(),
            Some("synthetic-recovered-refresh")
        );
        token_exchange.assert_async().await;
        profile_request.assert_async().await;
        restored_token.assert_async().await;
        restored_profile_request.assert_async().await;
    }

    #[tokio::test]
    async fn permission_renewal_cancels_before_commit_and_commits_after_cancellation() {
        let server = MockServer::start_async().await;
        let initial_refresh = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/token")
                    .body_includes("grant_type=refresh_token")
                    .body_includes("refresh_token=synthetic-old-refresh");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(
                        r#"{"token_type":"Bearer","access_token":"synthetic-initial-access","expires_in":3600,"refresh_token":"synthetic-current-refresh","scope":"Calendars.Read Mail.Read User.Read offline_access"}"#,
                    );
            })
            .await;
        let initial_profile_request = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/v1.0/me")
                    .header("authorization", "Bearer synthetic-initial-access");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(r#"{"id":"account-123","displayName":"Ada Example"}"#);
            })
            .await;
        let token_exchange = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/token")
                    .body_includes("grant_type=authorization_code");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(
                        r#"{"token_type":"Bearer","access_token":"synthetic-renewed-access","expires_in":3600,"refresh_token":"synthetic-renewed-refresh","scope":"User.Read offline_access"}"#,
                    );
            })
            .await;
        let renewed_profile_request = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/v1.0/me")
                    .header("authorization", "Bearer synthetic-renewed-access");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(r#"{"id":"account-123","displayName":"Ada Example"}"#);
            })
            .await;
        let restored_token = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/token")
                    .body_includes("grant_type=refresh_token")
                    .body_includes("refresh_token=synthetic-renewed-refresh");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(
                        r#"{"token_type":"Bearer","access_token":"synthetic-restored-access","expires_in":3600,"refresh_token":"synthetic-final-refresh","scope":"Calendars.Read Mail.Read User.Read offline_access"}"#,
                    );
            })
            .await;
        let restored_profile_request = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/v1.0/me")
                    .header("authorization", "Bearer synthetic-restored-access");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(r#"{"id":"account-123","displayName":"Ada Example"}"#);
            })
            .await;
        let (browser_sender, mut browser_receiver) = mpsc::unbounded_channel();
        let credentials = Arc::new(MemoryRefreshTokenStore::new(Some(refresh_token(
            "synthetic-old-refresh",
        ))));
        let provider = synthetic_provider(&server, browser_sender, credentials.clone());
        let workflow = Arc::new(AccountWorkflow::new(provider));
        let active_profile = workflow
            .restore_account()
            .await
            .expect("initial account restores")
            .expect("synthetic account is present");
        let expected_account_id = active_profile.id().clone();

        let cancelled_operation = AccountOperationCancellation::new();
        let task_cancellation = cancelled_operation.clone();
        let task_workflow = Arc::clone(&workflow);
        let cancelled_account_id = expected_account_id.clone();
        let cancelled_renewal = tokio::spawn(async move {
            task_workflow
                .reauthorize_account_with_cancellation(&cancelled_account_id, task_cancellation)
                .await
        });
        let _authorization_url = browser_receiver
            .recv()
            .await
            .expect("permission review opens the test browser");
        cancelled_operation.cancel();
        assert_eq!(
            cancelled_renewal
                .await
                .expect("cancelled permission review completes"),
            Err(AccountOperationError::SignInCancelled)
        );
        assert_eq!(
            credentials.token_value().as_deref(),
            Some("synthetic-current-refresh")
        );

        let (save_started, finish_save) = credentials.pause_next_save_after_write();
        let cancellation = AccountOperationCancellation::new();
        let task_cancellation = cancellation.clone();
        let task_workflow = Arc::clone(&workflow);
        let renewal_account_id = expected_account_id.clone();
        let renewal = tokio::spawn(async move {
            task_workflow
                .reauthorize_account_with_cancellation(&renewal_account_id, task_cancellation)
                .await
        });
        complete_test_authorization(&mut browser_receiver, "synthetic-renewal-code").await;
        tokio::time::timeout(Duration::from_secs(2), save_started)
            .await
            .expect("replacement credential commit begins")
            .expect("synthetic store signals its pending write");
        cancellation.cancel();
        finish_save.wait();

        let renewed_profile = renewal
            .await
            .expect("permission workflow completes")
            .expect("commit wins the cancellation race");
        assert_eq!(renewed_profile.id(), active_profile.id());
        assert_eq!(
            credentials.token_value().as_deref(),
            Some("synthetic-renewed-refresh")
        );

        let (restored_browser_sender, _) = mpsc::unbounded_channel();
        let restored_provider =
            synthetic_provider(&server, restored_browser_sender, credentials.clone());
        let restored_workflow = AccountWorkflow::new(restored_provider);
        let restored_profile = restored_workflow
            .restore_account()
            .await
            .expect("new workflow restores the renewed account")
            .expect("renewed account remains present");
        assert_eq!(restored_profile.id(), renewed_profile.id());
        assert_eq!(
            credentials.token_value().as_deref(),
            Some("synthetic-final-refresh")
        );
        initial_refresh.assert_async().await;
        initial_profile_request.assert_async().await;
        token_exchange.assert_async().await;
        renewed_profile_request.assert_async().await;
        restored_token.assert_async().await;
        restored_profile_request.assert_async().await;
    }
}
