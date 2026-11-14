use std::{
    net::{Ipv4Addr, SocketAddrV4},
    sync::{
        Arc, Barrier, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
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

use crate::test_support::{http_base_url, http_url, synthetic_access_token as access_token};
use crate::{
    CallbackPort,
    auth::{
        credential::{
            CredentialIdentity, RefreshTokenStore, SavedRefreshToken,
            SecretServiceCredentialCleanup, SecretServiceRefreshTokenStore,
        },
        identity::{BrowserLauncher, Endpoints},
    },
    error::GraphError,
    transport::{GraphAuthority, GraphTransport},
};
use einfach_mail_application::{
    AccountOperationCancellation, AccountOperationError, AccountPermissionRequest, AccountProvider,
    AccountProviderKind, AccountWorkflow, CalendarProvider, ConnectedAccount, MailOperationError,
    MailProvider, MailSendProviderError, MessageDeltaRequest, MessageDeltaResponse,
};
use einfach_mail_domain::{
    AccountId, CalendarEventCreation, CalendarEventDraft, CalendarOccurrenceTime,
    CalendarViewRange, CalendarViewReference, FolderId, FolderReference, MailFolderKind,
    MailReplyDraft, MailReplySource, MessageBodyType, MessageDeltaChange, MessageDeltaLink,
    MessageDeltaProgress, MessageId,
};
use time::{Date, Month, PrimitiveDateTime, Time};

use super::{
    MicrosoftGraphProvider, ProviderConfig, ProviderSession,
    session::invalidate_rejected_access_token,
};

struct MemoryRefreshTokenStore {
    credential: Mutex<Option<SavedRefreshToken>>,
    fail_next_delete: AtomicBool,
    save_pause: Mutex<Option<(Arc<Barrier>, oneshot::Sender<()>)>>,
}

impl MemoryRefreshTokenStore {
    fn new(token: Option<RefreshToken>) -> Self {
        Self {
            credential: Mutex::new(token.map(|token| {
                SavedRefreshToken::with_identity(
                    CredentialIdentity::Unverified {
                        expected_account_id: None,
                    },
                    token,
                )
            })),
            fail_next_delete: AtomicBool::new(false),
            save_pause: Mutex::new(None),
        }
    }

    fn with_verified_token(token: RefreshToken, account_id: AccountId) -> Self {
        let store = Self::new(None);
        *store
            .credential
            .lock()
            .expect("synthetic credential mutex is not poisoned") = Some(
            SavedRefreshToken::with_identity(CredentialIdentity::Verified(account_id), token),
        );
        store
    }

    fn fail_next_delete(&self) {
        self.fail_next_delete.store(true, Ordering::SeqCst);
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
        self.credential
            .lock()
            .expect("synthetic credential mutex is not poisoned")
            .as_ref()
            .map(|credential| credential.token().token().expose_secret().to_owned())
    }
}

impl RefreshTokenStore for MemoryRefreshTokenStore {
    fn load(&self) -> Result<Option<SavedRefreshToken>, AccountOperationError> {
        self.credential
            .lock()
            .map(|token| token.clone())
            .map_err(|_| AccountOperationError::CredentialStoreUnavailable)
    }

    fn save(
        &self,
        identity: &CredentialIdentity,
        token: &RefreshToken,
    ) -> Result<(), AccountOperationError> {
        {
            let mut saved = self
                .credential
                .lock()
                .map_err(|_| AccountOperationError::CredentialStoreUnavailable)?;
            *saved = Some(SavedRefreshToken::with_identity(
                identity.clone(),
                token.clone(),
            ));
        }
        let save_pause = self
            .save_pause
            .lock()
            .map_err(|_| AccountOperationError::CredentialStoreUnavailable)?
            .take();
        if let Some((barrier, started_sender)) = save_pause {
            started_sender
                .send(())
                .expect("the credential-save test is waiting at the barrier");
            barrier.wait();
        }
        Ok(())
    }

    fn delete(&self) -> Result<(), AccountOperationError> {
        if self.fail_next_delete.swap(false, Ordering::SeqCst) {
            return Err(AccountOperationError::CredentialStoreUnavailable);
        }
        self.credential
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

fn synthetic_provider<C>(
    server: &MockServer,
    sender: mpsc::UnboundedSender<Url>,
    credentials: Arc<C>,
) -> MicrosoftGraphProvider
where
    C: RefreshTokenStore + 'static,
{
    let endpoints = Endpoints {
        authorization: Url::parse(&http_url(server, "/authorize"))
            .expect("synthetic authorization endpoint is valid"),
        token: Url::parse(&http_url(server, "/token")).expect("synthetic token endpoint is valid"),
        graph_me: Url::parse(&format!(
            "{}?$select=id,displayName,mail,userPrincipalName",
            http_url(server, "/v1.0/me")
        ))
        .expect("synthetic Graph endpoint is valid"),
    };
    let config = ProviderConfig {
        client_id: Some(
            uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000001")
                .expect("synthetic client ID is valid"),
        ),
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

async fn connect_with_secret_service(
    server: &MockServer,
    credentials: Arc<SecretServiceRefreshTokenStore>,
) -> (MicrosoftGraphProvider, ConnectedAccount) {
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
                .body(SYNTHETIC_PROFILE_RESPONSE);
        })
        .await;
    let (browser_sender, mut browser_receiver) = mpsc::unbounded_channel();
    let provider = synthetic_provider(server, browser_sender, Arc::clone(&credentials));
    let workflow = Arc::new(AccountWorkflow::new(provider.clone()));
    let connecting_workflow = Arc::clone(&workflow);
    let connect_task = tokio::spawn(async move { connecting_workflow.connect_account().await });

    complete_test_authorization(&mut browser_receiver, "synthetic-authorization-code").await;
    let profile = connect_task
        .await
        .expect("synthetic provider connection completes")
        .expect("synthetic account connection succeeds");
    token_exchange.assert_async().await;
    profile_request.assert_async().await;

    (provider, profile)
}

async fn restore_with_secret_service(
    server: &MockServer,
    credentials: Arc<SecretServiceRefreshTokenStore>,
) -> (MicrosoftGraphProvider, ConnectedAccount) {
    let token_exchange = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/token")
                .body_includes("grant_type=refresh_token")
                .body_includes("refresh_token=synthetic-connect-refresh");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"token_type":"Bearer","access_token":"synthetic-restore-access","expires_in":3600,"refresh_token":"synthetic-rotated-refresh","scope":"Calendars.Read Mail.Read User.Read offline_access"}"#,
                );
        })
        .await;
    let profile_request = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me")
                .header("authorization", "Bearer synthetic-restore-access");
            then.status(200)
                .header("content-type", "application/json")
                .body(SYNTHETIC_PROFILE_RESPONSE);
        })
        .await;
    let (browser_sender, _browser_receiver) = mpsc::unbounded_channel();
    let restored_provider = synthetic_provider(server, browser_sender, Arc::clone(&credentials));
    let profile = restored_provider
        .restore_account()
        .await
        .expect("saved account restores with the mock provider")
        .expect("verified synthetic account is present");

    token_exchange.assert_async().await;
    profile_request.assert_async().await;

    (restored_provider, profile)
}

const SYNTHETIC_PROFILE_RESPONSE: &str = r#"{"id":"synthetic-account","displayName":"Synthetic Account","mail":"synthetic@example.test","userPrincipalName":"synthetic@tenant.example.test"}"#;

fn assert_saved_refresh_token(
    credentials: &SecretServiceRefreshTokenStore,
    expected_account_id: &AccountId,
    expected_token: &str,
) {
    let saved_credential = credentials
        .load()
        .expect("synthetic Secret Service credential loads")
        .expect("refresh token is present");

    assert_eq!(saved_credential.account_id(), Some(expected_account_id));
    assert_eq!(
        saved_credential.expected_account_id(),
        Some(expected_account_id)
    );
    assert_eq!(
        saved_credential.token().token().expose_secret(),
        expected_token
    );
}

include!("tests/connection.rs");
include!("tests/restore.rs");
include!("tests/permissions.rs");
include!("tests/mail.rs");
include!("tests/calendar.rs");
