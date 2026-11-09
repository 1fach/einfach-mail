use std::{
    net::{Ipv4Addr, SocketAddrV4},
    sync::{
        Arc, Barrier, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime},
};

use httpmock::{
    Method::{GET, POST},
    MockServer,
};
use huskarl::{
    core::secrets::SecretString,
    token::{AccessToken, BearerAccessToken, RefreshToken},
};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use url::Url;

use crate::{
    CallbackPort,
    credential::{
        CredentialIdentity, RefreshTokenStore, SavedRefreshToken, SecretServiceCredentialCleanup,
        SecretServiceRefreshTokenStore,
    },
    error::GraphError,
    graph_url::GraphAuthority,
    identity::{BrowserLauncher, Endpoints},
    transport::GraphTransport,
};
use einfach_mail_application::{
    AccountOperationCancellation, AccountOperationError, AccountPermissionRequest, AccountProvider,
    AccountWorkflow, CalendarProvider, MailOperationError, MailProvider, MailSendProviderError,
    MessageDeltaRequest, MessageDeltaResponse, WellKnownMailFolder,
};
use einfach_mail_domain::{
    AccountId, AccountProfile, CalendarEventCreation, CalendarEventDraft, CalendarOccurrenceTime,
    CalendarViewRange, CalendarViewReference, FolderId, FolderReference, MailReplyDraft,
    MailReplySource, MessageBodyType, MessageDeltaChange, MessageDeltaLink, MessageDeltaProgress,
    MessageId,
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

fn access_token(token_text: &str) -> Arc<AccessToken> {
    Arc::new(AccessToken::Bearer(BearerAccessToken::new(
        SecretString::new(token_text),
        SystemTime::now(),
        Some(Duration::from_secs(3_600)),
    )))
}

fn http_url(server: &MockServer, path: &str) -> String {
    format!("http://{}{path}", server.address())
}

fn http_base_url(server: &MockServer) -> String {
    http_url(server, "")
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

#[tokio::test]
async fn reply_to_message_preserves_accepted_and_unknown_post_outcomes() {
    let server = MockServer::start_async().await;
    let accepted_reply = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/v1.0/me/messages/source%2Fmessage/reply")
                .header("authorization", "Bearer synthetic-send-access")
                .header("content-type", "application/json")
                .body(
                    r#"{"message":{"body":{"contentType":"Text","content":"Synthetic reply body"}}}"#,
                );
            then.status(202);
        })
        .await;
    let ambiguous_reply = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/v1.0/me/messages/source%2Fmessage/reply")
                .body(
                    r#"{"message":{"body":{"contentType":"Text","content":"Synthetic uncertain reply body"}}}"#,
                );
            then.status(500);
        })
        .await;
    let (browser_sender, _browser_receiver) = mpsc::unbounded_channel();
    let credentials = Arc::new(MemoryRefreshTokenStore::new(None));
    let provider = synthetic_provider(&server, browser_sender, credentials);
    let account =
        AccountId::try_from("account-123".to_owned()).expect("synthetic account ID is valid");
    *provider.config.session.lock().await = Some(ProviderSession {
        account_id: account.clone(),
        access_token: Some(access_token("synthetic-send-access")),
    });
    let folder = FolderReference::new(
        account.clone(),
        FolderId::try_from("inbox".to_owned()).expect("synthetic folder ID is valid"),
    );
    let source = MailReplySource::new(
        folder,
        MessageId::try_from("source/message".to_owned()).expect("synthetic message ID is valid"),
    );
    let draft = MailReplyDraft::new(source.clone(), "Synthetic reply body".to_owned());

    assert_eq!(provider.reply_to_message(&account, draft).await, Ok(()));
    assert_eq!(
        provider
            .reply_to_message(
                &account,
                MailReplyDraft::new(source, "Synthetic uncertain reply body".to_owned(),),
            )
            .await,
        Err(MailSendProviderError::OutcomeUnknown)
    );
    accepted_reply.assert_calls_async(1).await;
    ambiguous_reply.assert_calls_async(1).await;
}

#[tokio::test]
async fn reply_to_message_rejects_another_account_before_request() {
    let server = MockServer::start_async().await;
    let unexpected_reply = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/v1.0/me/messages/source-message/reply");
            then.status(202);
        })
        .await;
    let (browser_sender, _browser_receiver) = mpsc::unbounded_channel();
    let provider = synthetic_provider(
        &server,
        browser_sender,
        Arc::new(MemoryRefreshTokenStore::new(None)),
    );
    let account =
        AccountId::try_from("account-123".to_owned()).expect("synthetic account ID is valid");
    *provider.config.session.lock().await = Some(ProviderSession {
        account_id: account.clone(),
        access_token: Some(access_token("synthetic-send-access")),
    });
    let other_account =
        AccountId::try_from("account-456".to_owned()).expect("synthetic account ID is valid");
    let folder = FolderReference::new(
        other_account,
        FolderId::try_from("inbox".to_owned()).expect("synthetic folder ID is valid"),
    );
    let source = MailReplySource::new(
        folder,
        MessageId::try_from("source-message".to_owned()).expect("synthetic message ID is valid"),
    );

    assert_eq!(
        provider
            .reply_to_message(&account, MailReplyDraft::new(source, "reply".to_owned()))
            .await,
        Err(MailSendProviderError::AccountMismatch)
    );
    unexpected_reply.assert_calls_async(0).await;
}

#[tokio::test]
async fn message_move_is_account_scoped_and_uses_graph_confirmation() {
    let server = MockServer::start_async().await;
    let move_request = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/v1.0/me/messages/message-123/move")
                .header("authorization", "Bearer synthetic-move-access")
                .header("prefer", "IdType=\"ImmutableId\"")
                .body(r#"{"destinationId":"archive"}"#);
            then.status(201)
                .header("content-type", "application/json")
                .body(r#"{"id":"message-123","parentFolderId":"archive"}"#);
        })
        .await;
    let (browser_sender, _browser_receiver) = mpsc::unbounded_channel();
    let provider = synthetic_provider(
        &server,
        browser_sender,
        Arc::new(MemoryRefreshTokenStore::new(None)),
    );
    let account =
        AccountId::try_from("account-123".to_owned()).expect("synthetic account ID is valid");
    *provider.config.session.lock().await = Some(ProviderSession {
        account_id: account.clone(),
        access_token: Some(access_token("synthetic-move-access")),
    });
    let source = FolderReference::new(
        account.clone(),
        FolderId::try_from("inbox".to_owned()).expect("synthetic folder ID is valid"),
    );
    let destination = FolderReference::new(
        account,
        FolderId::try_from("archive".to_owned()).expect("synthetic folder ID is valid"),
    );
    let message_id =
        MessageId::try_from("message-123".to_owned()).expect("synthetic message ID is valid");

    assert_eq!(
        provider
            .move_message(&source, &message_id, &destination)
            .await,
        Ok(())
    );
    let other_account =
        AccountId::try_from("other-account".to_owned()).expect("synthetic account ID is valid");
    let other_destination = FolderReference::new(
        other_account,
        FolderId::try_from("archive".to_owned()).expect("synthetic folder ID is valid"),
    );
    assert_eq!(
        provider
            .move_message(&source, &message_id, &other_destination)
            .await,
        Err(MailOperationError::AccountMismatch)
    );
    move_request.assert_calls_async(1).await;
}

#[tokio::test]
async fn well_known_folder_lookup_is_account_scoped() {
    let server = MockServer::start_async().await;
    let lookup = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/mailFolders/deleteditems")
                .query_param("$select", "id,displayName,isHidden")
                .header("authorization", "Bearer synthetic-deleted-access");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"deleted-folder","displayName":"Gelöschte Elemente","isHidden":false}"#);
        })
        .await;
    let (browser_sender, _browser_receiver) = mpsc::unbounded_channel();
    let provider = synthetic_provider(
        &server,
        browser_sender,
        Arc::new(MemoryRefreshTokenStore::new(None)),
    );
    let account =
        AccountId::try_from("account-123".to_owned()).expect("synthetic account ID is valid");
    *provider.config.session.lock().await = Some(ProviderSession {
        account_id: account.clone(),
        access_token: Some(access_token("synthetic-deleted-access")),
    });

    let folder = provider
        .well_known_folder(&account, WellKnownMailFolder::DeletedItems)
        .await
        .expect("synthetic Deleted Items folder resolves");
    let other_account =
        AccountId::try_from("other-account".to_owned()).expect("synthetic account ID is valid");

    assert_eq!(folder.account_id(), &account);
    assert_eq!(folder.folder_id().as_str(), "deleted-folder");
    assert_eq!(
        provider
            .well_known_folder(&other_account, WellKnownMailFolder::DeletedItems)
            .await,
        Err(MailOperationError::AccountMismatch)
    );
    lookup.assert_calls_async(1).await;
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
    let workflow = Arc::new(AccountWorkflow::new(provider.clone()));
    let connect_workflow = Arc::clone(&workflow);
    let connect_task = tokio::spawn(async move { connect_workflow.connect_account().await });
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
    assert!(!has_scope(&parameters, "Mail.ReadWrite"));
    assert!(!has_scope(&parameters, "Calendars.ReadWrite"));
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

    credentials.fail_next_delete();
    assert_eq!(
        workflow.sign_out().await,
        Err(AccountOperationError::CredentialStoreUnavailable)
    );
    assert_eq!(
        credentials.token_value().as_deref(),
        Some("synthetic-refresh")
    );
    assert_eq!(
        workflow.connect_account().await,
        Err(AccountOperationError::AccountAlreadyConnected)
    );
    assert!(
        provider
            .list_folders(profile.id())
            .await
            .expect("provider session remains available for retry")
            .is_empty()
    );

    workflow
        .sign_out()
        .await
        .expect("retry removes the synthetic credential");
    assert_eq!(credentials.token_value(), None);
}

#[tokio::test]
async fn connect_rejects_a_forged_callback_state_before_token_exchange() {
    let server = MockServer::start_async().await;
    let unexpected_token_exchange = server
        .mock_async(|when, then| {
            when.method(POST).path("/token");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"token_type":"Bearer","access_token":"synthetic-access","expires_in":3600,"refresh_token":"synthetic-refresh"}"#,
                );
        })
        .await;
    let (browser_sender, mut browser_receiver) = mpsc::unbounded_channel();
    let credentials = Arc::new(MemoryRefreshTokenStore::new(None));
    let provider = synthetic_provider(&server, browser_sender, credentials.clone());
    let connect_task = tokio::spawn(async move {
        provider
            .connect_account(AccountOperationCancellation::new())
            .await
    });
    let authorization_url = browser_receiver
        .recv()
        .await
        .expect("sign-in opens the test browser");
    let mut callback_url =
        callback_url_for_response(&authorization_url, "code", "synthetic-authorization-code");
    callback_url
        .query_pairs_mut()
        .clear()
        .append_pair("code", "synthetic-authorization-code")
        .append_pair("state", "forged-state");
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("test HTTP client is valid")
        .get(callback_url)
        .send()
        .await
        .expect("forged callback reaches the loopback listener");

    assert_eq!(
        connect_task.await.expect("provider task completes"),
        Err(AccountOperationError::SignInFailed)
    );
    assert_eq!(credentials.token_value(), None);
    unexpected_token_exchange.assert_calls_async(0).await;
}

#[tokio::test]
async fn connect_does_not_follow_a_token_endpoint_redirect() {
    let server = MockServer::start_async().await;
    let redirect_target_url = http_url(&server, "/token-redirect-target");
    let token_redirect = server
        .mock_async(|when, then| {
            when.method(POST).path("/token");
            then.status(302).header("location", redirect_target_url);
        })
        .await;
    let redirect_target = server
        .mock_async(|when, then| {
            when.path("/token-redirect-target");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"token_type":"Bearer","access_token":"synthetic-access","expires_in":3600,"refresh_token":"synthetic-refresh"}"#,
                );
        })
        .await;
    let (browser_sender, mut browser_receiver) = mpsc::unbounded_channel();
    let credentials = Arc::new(MemoryRefreshTokenStore::new(None));
    let provider = synthetic_provider(&server, browser_sender, credentials.clone());
    let connect_task = tokio::spawn(async move {
        provider
            .connect_account(AccountOperationCancellation::new())
            .await
    });
    let authorization_url = browser_receiver
        .recv()
        .await
        .expect("sign-in opens the test browser");
    let callback_response = reqwest::Client::builder()
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

    assert!(callback_response.status().is_success());
    assert!(
        connect_task
            .await
            .expect("provider task completes")
            .is_err()
    );
    assert_eq!(credentials.token_value(), None);
    token_redirect.assert_calls_async(1).await;
    redirect_target.assert_calls_async(0).await;
}

#[tokio::test]
#[ignore = "requires an unlocked Linux Secret Service session"]
async fn provider_credential_lifecycle_uses_secret_service_across_restore() {
    let server = MockServer::start_async().await;
    let credentials = Arc::new(SecretServiceRefreshTokenStore::with_account(format!(
        "provider-test-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    )));
    let _cleanup = SecretServiceCredentialCleanup::new(credentials.as_ref().clone());
    let (provider, profile) = connect_with_secret_service(&server, Arc::clone(&credentials)).await;
    let account_id = profile.id().clone();

    assert_eq!(account_id.as_str(), "synthetic-account");
    assert_saved_refresh_token(&credentials, &account_id, "synthetic-connect-refresh");
    drop(provider);

    let (provider, restored_profile) =
        restore_with_secret_service(&server, Arc::clone(&credentials)).await;

    assert_eq!(restored_profile.id(), &account_id);
    assert_saved_refresh_token(&credentials, &account_id, "synthetic-rotated-refresh");
    provider
        .sign_out()
        .await
        .expect("sign-out removes the saved Secret Service credential");
    assert!(
        credentials
            .load()
            .expect("credential lookup succeeds")
            .is_none()
    );
}

async fn connect_with_secret_service(
    server: &MockServer,
    credentials: Arc<SecretServiceRefreshTokenStore>,
) -> (MicrosoftGraphProvider, AccountProfile) {
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
) -> (MicrosoftGraphProvider, AccountProfile) {
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

#[tokio::test]
async fn mail_provider_reads_scoped_folders_delta_and_body() {
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
    let next_link = format!(
        "{}/v1.0/me/mailFolders/folder-inbox/messages/delta?$skiptoken=opaque-next",
        http_base_url(&server)
    );
    let delta_link = format!(
        "{}/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken=opaque-final",
        http_base_url(&server)
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
                    "value": [{
                        "id": "message-1",
                        "subject": "Synthetic message",
                        "receivedDateTime": "2026-10-31T12:00:00Z",
                        "isRead": false
                    }],
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
    let expected_account_id =
        AccountId::try_from("account-mail".to_owned()).expect("synthetic ID is valid");
    let credentials = Arc::new(MemoryRefreshTokenStore::with_verified_token(
        refresh_token("synthetic-old-refresh"),
        expected_account_id.clone(),
    ));
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
    let response = provider
        .message_delta_page(&folder, MessageDeltaRequest::Initial)
        .await
        .expect("provider loads one synthetic delta page");
    let MessageDeltaResponse::Page(page) = response else {
        panic!("synthetic provider response is a page");
    };
    let (page, response_bytes) = page.into_parts();
    assert!(response_bytes > 0);
    let [MessageDeltaChange::Upsert { id: message_id, .. }] = page.changes() else {
        panic!("synthetic initial delta contains one message upsert");
    };
    let message_id = message_id.clone();
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
        .fetch_message_body(&folder, &message_id)
        .await
        .expect("provider loads the requested synthetic body");
    assert_eq!(body.content_type(), MessageBodyType::Html);
    assert_eq!(body.content(), "<p>Safe fixture</p>");

    let another_account = AccountId::try_from("another-account".to_owned())
        .expect("synthetic account ID is non-empty");
    let another_folder = FolderReference::new(another_account, folder.folder_id().clone());
    assert_eq!(
        provider
            .message_delta_page(&another_folder, MessageDeltaRequest::Initial)
            .await,
        Err(MailOperationError::AccountMismatch),
    );

    token_refresh.assert_async().await;
    profile_request.assert_async().await;
    root_folders.assert_async().await;
    child_folders.assert_async().await;
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
    let existing_account_id =
        AccountId::try_from("account-123".to_owned()).expect("synthetic account ID is valid");
    let credentials = Arc::new(MemoryRefreshTokenStore::with_verified_token(
        refresh_token("synthetic-existing-refresh"),
        existing_account_id,
    ));
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
                    r#"{"token_type":"Bearer","access_token":"synthetic-renewed-access","expires_in":3600,"refresh_token":"synthetic-rotated-refresh","scope":"Calendars.ReadWrite Mail.Read User.Read offline_access"}"#,
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
    let saved_credential = credentials
        .load()
        .expect("synthetic credential can be read")
        .expect("verified credential remains saved");
    assert_eq!(saved_credential.account_id(), Some(profile.id()));
    assert_eq!(saved_credential.expected_account_id(), Some(profile.id()));
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
                    r#"{"token_type":"Bearer","access_token":"synthetic-rejected-access","expires_in":3600,"refresh_token":"synthetic-rotated-refresh","scope":"Calendars.ReadWrite Mail.Read User.Read offline_access"}"#,
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
    let expected_account_id =
        AccountId::try_from("saved-account".to_owned()).expect("synthetic ID is valid");
    let credentials = Arc::new(MemoryRefreshTokenStore::with_verified_token(
        refresh_token("synthetic-old-refresh"),
        expected_account_id.clone(),
    ));
    let provider = synthetic_provider(&server, browser_sender, credentials.clone());

    assert_eq!(
        provider.restore_account().await,
        Err(AccountOperationError::AccessTokenRejected)
    );
    assert_eq!(
        credentials.token_value().as_deref(),
        Some("synthetic-rotated-refresh")
    );
    let pending_identity = credentials
        .load()
        .expect("synthetic credential can be read")
        .expect("pending credential remains saved");
    assert!(pending_identity.account_id().is_none());
    assert_eq!(
        pending_identity.expected_account_id(),
        Some(&expected_account_id)
    );
    token_refresh.assert_async().await;
    profile_request.assert_async().await;
}

#[tokio::test]
async fn rejected_graph_result_only_invalidates_the_request_token() {
    let server = MockServer::start_async().await;
    let (browser_sender, _browser_receiver) = mpsc::unbounded_channel();
    let credentials = Arc::new(MemoryRefreshTokenStore::new(None));
    let provider = synthetic_provider(&server, browser_sender, credentials);
    let account_id =
        AccountId::try_from("account-123".to_owned()).expect("synthetic account ID is valid");
    let rejected_token = access_token("synthetic-rejected-access");
    *provider.config.session.lock().await = Some(ProviderSession {
        account_id: account_id.clone(),
        access_token: Some(Arc::clone(&rejected_token)),
    });

    let rejected_result: Result<(), GraphError> = Err(GraphError::AccessTokenRejected);
    assert!(matches!(
        invalidate_rejected_access_token(
            &provider.config,
            &account_id,
            &rejected_token,
            rejected_result,
        )
        .await,
        Err(GraphError::AccessTokenRejected)
    ));
    let cleared_session = provider.config.session.lock().await;
    assert!(cleared_session.as_ref().is_some_and(|session| {
        session.account_id == account_id && session.access_token.is_none()
    }));
    drop(cleared_session);

    let current_token = access_token("synthetic-current-access");
    *provider.config.session.lock().await = Some(ProviderSession {
        account_id: account_id.clone(),
        access_token: Some(Arc::clone(&current_token)),
    });
    let delayed_rejection: Result<(), GraphError> = Err(GraphError::AccessTokenRejected);
    assert!(matches!(
        invalidate_rejected_access_token(
            &provider.config,
            &account_id,
            &rejected_token,
            delayed_rejection,
        )
        .await,
        Err(GraphError::AccessTokenRejected)
    ));
    let preserved_session = provider.config.session.lock().await;
    assert!(preserved_session.as_ref().is_some_and(|session| {
        session
            .access_token
            .as_ref()
            .is_some_and(|cached| Arc::ptr_eq(cached, &current_token))
    }));
    drop(preserved_session);

    let unrelated_error: Result<(), GraphError> = Err(GraphError::AccessDenied);
    assert!(matches!(
        invalidate_rejected_access_token(
            &provider.config,
            &account_id,
            &current_token,
            unrelated_error,
        )
        .await,
        Err(GraphError::AccessDenied)
    ));
    let successful_result: Result<usize, GraphError> = Ok(42);
    assert!(matches!(
        invalidate_rejected_access_token(
            &provider.config,
            &account_id,
            &current_token,
            successful_result,
        )
        .await,
        Ok(42)
    ));
    let preserved_session = provider.config.session.lock().await;
    assert!(preserved_session.as_ref().is_some_and(|session| {
        session
            .access_token
            .as_ref()
            .is_some_and(|cached| Arc::ptr_eq(cached, &current_token))
    }));
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
async fn restore_mismatch_remains_pending_after_provider_restart() {
    let server = MockServer::start_async().await;
    let first_refresh = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/token")
                .body_includes("refresh_token=synthetic-original-refresh");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"token_type":"Bearer","access_token":"synthetic-first-access","expires_in":3600,"refresh_token":"synthetic-first-rotated","scope":"User.Read offline_access"}"#,
                );
        })
        .await;
    let second_refresh = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/token")
                .body_includes("refresh_token=synthetic-first-rotated");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"token_type":"Bearer","access_token":"synthetic-second-access","expires_in":3600,"refresh_token":"synthetic-second-rotated","scope":"User.Read offline_access"}"#,
                );
        })
        .await;
    let first_profile = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me")
                .header("authorization", "Bearer synthetic-first-access");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"returned-account","displayName":"Ada Example"}"#);
        })
        .await;
    let second_profile = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me")
                .header("authorization", "Bearer synthetic-second-access");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"returned-account","displayName":"Ada Example"}"#);
        })
        .await;
    let expected_account_id =
        AccountId::try_from("expected-account".to_owned()).expect("synthetic ID is valid");
    let credentials = Arc::new(MemoryRefreshTokenStore::with_verified_token(
        refresh_token("synthetic-original-refresh"),
        expected_account_id.clone(),
    ));
    let (browser_sender, _) = mpsc::unbounded_channel();
    let provider = synthetic_provider(&server, browser_sender, credentials.clone());

    assert_eq!(
        provider.restore_account().await,
        Err(AccountOperationError::AccountMismatch)
    );
    let pending_after_mismatch = credentials
        .load()
        .expect("pending credential can be read")
        .expect("rotated credential remains saved");
    assert_eq!(pending_after_mismatch.account_id(), None);
    assert_eq!(
        pending_after_mismatch.expected_account_id(),
        Some(&expected_account_id)
    );
    assert_eq!(
        credentials.token_value().as_deref(),
        Some("synthetic-first-rotated")
    );
    assert!(provider.config.session.lock().await.is_none());

    let (restart_browser_sender, _) = mpsc::unbounded_channel();
    let restarted_provider =
        synthetic_provider(&server, restart_browser_sender, credentials.clone());
    assert_eq!(
        restarted_provider.restore_account().await,
        Err(AccountOperationError::AccountMismatch)
    );
    assert_eq!(
        restarted_provider.sign_out().await,
        Err(AccountOperationError::SavedAccountIdentityUnavailable)
    );
    let pending_after_restart = credentials
        .load()
        .expect("pending credential can be read after restart")
        .expect("pending credential remains saved after restart");
    assert_eq!(pending_after_restart.account_id(), None);
    assert_eq!(
        pending_after_restart.expected_account_id(),
        Some(&expected_account_id)
    );
    assert_eq!(
        credentials.token_value().as_deref(),
        Some("synthetic-second-rotated")
    );
    first_refresh.assert_async().await;
    second_refresh.assert_async().await;
    first_profile.assert_async().await;
    second_profile.assert_async().await;
}

#[tokio::test]
async fn unverified_credential_blocks_replacement_and_sign_out() {
    let server = MockServer::start_async().await;
    let (browser_sender, mut browser_receiver) = mpsc::unbounded_channel();
    let credentials = Arc::new(MemoryRefreshTokenStore::new(Some(refresh_token(
        "synthetic-legacy-refresh",
    ))));
    let provider = synthetic_provider(&server, browser_sender, credentials.clone());

    assert_eq!(
        provider
            .connect_account(AccountOperationCancellation::new())
            .await,
        Err(AccountOperationError::SavedAccountIdentityUnavailable)
    );
    assert!(browser_receiver.try_recv().is_err());
    assert_eq!(
        provider.sign_out().await,
        Err(AccountOperationError::SavedAccountIdentityUnavailable)
    );
    assert_eq!(
        credentials.token_value().as_deref(),
        Some("synthetic-legacy-refresh")
    );
    assert!(provider.config.session.lock().await.is_none());
}

#[tokio::test]
async fn routine_refresh_rejects_an_unverified_credential() {
    let server = MockServer::start_async().await;
    let refresh_request = server
        .mock_async(|when, then| {
            when.method(POST).path("/token");
            then.status(500);
        })
        .await;
    let (browser_sender, _) = mpsc::unbounded_channel();
    let credentials = Arc::new(MemoryRefreshTokenStore::new(Some(refresh_token(
        "synthetic-legacy-refresh",
    ))));
    let provider = synthetic_provider(&server, browser_sender, credentials);
    let account_id =
        AccountId::try_from("active-account".to_owned()).expect("synthetic ID is valid");
    *provider.config.session.lock().await = Some(ProviderSession {
        account_id: account_id.clone(),
        access_token: None,
    });

    assert_eq!(
        provider.list_folders(&account_id).await,
        Err(MailOperationError::AccountMismatch)
    );
    refresh_request.assert_calls_async(0).await;
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
async fn calendar_permission_renewal_requests_only_calendar_write_access() {
    let server = MockServer::start_async().await;
    let token_exchange = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/token")
                .body_includes("grant_type=authorization_code");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"token_type":"Bearer","access_token":"synthetic-calendar-write-access","expires_in":3600,"refresh_token":"synthetic-calendar-write-refresh","scope":"Calendars.ReadWrite Mail.Read User.Read offline_access"}"#,
                );
        })
        .await;
    let profile_request = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me")
                .header("authorization", "Bearer synthetic-calendar-write-access");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"account-123","displayName":"Ada Example"}"#);
        })
        .await;
    let (browser_sender, mut browser_receiver) = mpsc::unbounded_channel();
    let expected_account =
        AccountId::try_from("account-123".to_owned()).expect("synthetic account ID is valid");
    let credentials = Arc::new(MemoryRefreshTokenStore::with_verified_token(
        refresh_token("synthetic-old-refresh"),
        expected_account.clone(),
    ));
    let provider = synthetic_provider(&server, browser_sender, credentials.clone());
    *provider.config.session.lock().await = Some(ProviderSession {
        account_id: expected_account.clone(),
        access_token: Some(access_token("synthetic-old-access")),
    });

    let reauthorize_provider = provider.clone();
    let reauthorize_account = expected_account.clone();
    let reauthorization = tokio::spawn(async move {
        reauthorize_provider
            .reauthorize_account(
                &reauthorize_account,
                AccountPermissionRequest::CalendarReadWrite,
                AccountOperationCancellation::new(),
            )
            .await
    });
    let authorization_url = browser_receiver
        .recv()
        .await
        .expect("calendar permission review opens the test browser");
    let parameters = authorization_url
        .query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect::<std::collections::HashMap<_, _>>();
    assert!(has_scope(&parameters, "Calendars.ReadWrite"));
    assert!(!has_scope(&parameters, "Calendars.Read"));
    assert!(has_scope(&parameters, "Mail.Read"));
    assert!(!has_scope(&parameters, "Mail.ReadWrite"));
    assert!(!has_scope(&parameters, "Mail.Send"));

    let callback_response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("test HTTP client is valid")
        .get(callback_url_for_response(
            &authorization_url,
            "code",
            "synthetic-calendar-permission-code",
        ))
        .send()
        .await
        .expect("loopback callback reaches the test listener");
    assert!(callback_response.status().is_success());
    let profile = reauthorization
        .await
        .expect("calendar permission renewal task completes")
        .expect("same-account calendar permission renewal succeeds");
    assert_eq!(profile.id(), &expected_account);
    assert_eq!(
        credentials.token_value().as_deref(),
        Some("synthetic-calendar-write-refresh")
    );
    token_exchange.assert_async().await;
    profile_request.assert_async().await;
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
                    r#"{"token_type":"Bearer","access_token":"synthetic-renewed-access","expires_in":3600,"refresh_token":"synthetic-renewed-refresh","scope":"Calendars.Read Mail.ReadWrite User.Read offline_access"}"#,
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
                    r#"{"token_type":"Bearer","access_token":"synthetic-restored-access","expires_in":3600,"refresh_token":"synthetic-final-refresh","scope":"Calendars.Read Mail.ReadWrite User.Read offline_access"}"#,
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
            .reauthorize_account_with_cancellation(
                &cancelled_account_id,
                AccountPermissionRequest::MailReadWrite,
                task_cancellation,
            )
            .await
    });
    let authorization_url = browser_receiver
        .recv()
        .await
        .expect("permission review opens the test browser");
    let requested_scopes = authorization_url
        .query_pairs()
        .find_map(|(name, value)| (name == "scope").then(|| value.into_owned()))
        .expect("authorization request includes delegated scopes");
    let requested_scopes = requested_scopes
        .split_ascii_whitespace()
        .collect::<Vec<_>>();
    assert!(requested_scopes.contains(&"Mail.ReadWrite"));
    assert!(requested_scopes.contains(&"Calendars.Read"));
    assert!(requested_scopes.contains(&"User.Read"));
    assert!(requested_scopes.contains(&"offline_access"));
    assert!(!requested_scopes.contains(&"Mail.Read"));
    assert!(!requested_scopes.contains(&"Mail.Send"));
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
            .reauthorize_account_with_cancellation(
                &renewal_account_id,
                AccountPermissionRequest::MailReadWrite,
                task_cancellation,
            )
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

#[tokio::test]
async fn permission_renewal_for_another_account_keeps_the_saved_session() {
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
    let initial_profile = server
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
                    r#"{"token_type":"Bearer","access_token":"synthetic-other-access","expires_in":3600,"refresh_token":"synthetic-other-refresh","scope":"Calendars.Read Mail.ReadWrite User.Read offline_access"}"#,
                );
        })
        .await;
    let other_profile = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me")
                .header("authorization", "Bearer synthetic-other-access");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"account-456","displayName":"Grace Example"}"#);
        })
        .await;
    let (browser_sender, mut browser_receiver) = mpsc::unbounded_channel();
    let credentials = Arc::new(MemoryRefreshTokenStore::new(Some(refresh_token(
        "synthetic-old-refresh",
    ))));
    let provider = synthetic_provider(&server, browser_sender, credentials.clone());
    let active_profile = provider
        .restore_account()
        .await
        .expect("saved account restores")
        .expect("synthetic account is present");

    let expected_account_id = active_profile.id().clone();
    let cancellation = AccountOperationCancellation::new();
    let renewal = provider.reauthorize_account(
        &expected_account_id,
        AccountPermissionRequest::MailReadWrite,
        cancellation,
    );
    let complete_authorization = async {
        let authorization_url = browser_receiver
            .recv()
            .await
            .expect("permission review opens the test browser");
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("test HTTP client is valid")
            .get(callback_url_for_response(
                &authorization_url,
                "code",
                "synthetic-other-account-code",
            ))
            .send()
            .await
            .expect("loopback callback reaches the test listener");
        assert!(response.status().is_success());
    };
    let (renewal_result, ()) = tokio::join!(renewal, complete_authorization);

    assert_eq!(renewal_result, Err(AccountOperationError::AccountMismatch));
    assert_eq!(
        credentials.token_value().as_deref(),
        Some("synthetic-current-refresh")
    );
    let session = provider.config.session.lock().await;
    let session = session
        .as_ref()
        .expect("mismatched authorization preserves the active session");
    assert_eq!(session.account_id, *active_profile.id());
    assert_eq!(
        session
            .access_token
            .as_ref()
            .map(|token| token.token().expose_secret()),
        Some("synthetic-initial-access")
    );
    initial_refresh.assert_async().await;
    initial_profile.assert_async().await;
    token_exchange.assert_async().await;
    other_profile.assert_async().await;
}

#[tokio::test]
async fn create_event_renews_delegated_access_and_posts_one_timed_primary_calendar_event() {
    let server = MockServer::start_async().await;
    let token_refresh = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/token")
                .body_includes("grant_type=refresh_token");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"token_type":"Bearer","access_token":"synthetic-calendar-access","expires_in":3600,"refresh_token":"synthetic-calendar-refresh","scope":"Calendars.ReadWrite Mail.Read User.Read offline_access"}"#,
                );
        })
        .await;
    let profile_request = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me")
                .header("authorization", "Bearer synthetic-calendar-access");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"account-calendar","displayName":"Synthetic Calendar Account"}"#);
        })
        .await;
    let date =
        Date::from_calendar_date(2030, Month::January, 8).expect("synthetic event date is valid");
    let start = date
        .with_hms(9, 30, 0)
        .expect("synthetic event start is valid")
        .assume_utc();
    let end = date
        .with_hms(10, 0, 0)
        .expect("synthetic event end is valid")
        .assume_utc();
    let start_date =
        Date::from_calendar_date(2030, Month::January, 7).expect("synthetic week start is valid");
    let end_date =
        Date::from_calendar_date(2030, Month::January, 14).expect("synthetic week end is valid");
    let account =
        AccountId::try_from("account-calendar".to_owned()).expect("synthetic account ID is valid");
    let view = CalendarViewReference::new(
        account.clone(),
        CalendarViewRange::new(
            start_date.midnight().assume_utc(),
            end_date.midnight().assume_utc(),
            start_date,
            end_date,
        )
        .expect("synthetic calendar view is valid"),
    );
    let draft = CalendarEventDraft::timed(
        "Synthetic planning meeting".to_owned(),
        PrimitiveDateTime::new(
            date,
            Time::from_hms(9, 30, 0).expect("synthetic local start is valid"),
        ),
        PrimitiveDateTime::new(
            date,
            Time::from_hms(10, 0, 0).expect("synthetic local end is valid"),
        ),
        "Etc/UTC"
            .to_owned()
            .try_into()
            .expect("synthetic source zone is valid"),
        start,
        end,
    )
    .expect("synthetic event draft is valid");
    let creation = CalendarEventCreation::try_new(view.clone(), draft)
        .expect("synthetic event endpoints fit the selected week");
    let event_request = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/v1.0/me/calendar/events")
                .header("authorization", "Bearer synthetic-calendar-access")
                .header("content-type", "application/json")
                .header("prefer", "IdType=\"ImmutableId\"")
                .body_includes(r#""subject":"Synthetic planning meeting""#)
                .body_includes(r#""dateTime":"2030-01-08T09:30:00""#)
                .body_includes(r#""dateTime":"2030-01-08T10:00:00""#)
                .body_includes(r#""timeZone":"UTC""#)
                .body_includes(format!("\"transactionId\":\"{}\"", creation.id().as_uuid()));
            then.status(201)
                .header("content-type", "application/json")
                .body(format!(
                    r#"{{"id":"synthetic-created-event","subject":"Synthetic planning meeting","start":{{"dateTime":"2030-01-08T09:30:00","timeZone":"UTC"}},"end":{{"dateTime":"2030-01-08T10:00:00","timeZone":"UTC"}},"isAllDay":false,"transactionId":"{}"}}"#,
                    creation.id().as_uuid()
                ));
        })
        .await;
    let (browser_sender, _browser_receiver) = mpsc::unbounded_channel();
    let credentials = Arc::new(MemoryRefreshTokenStore::new(Some(refresh_token(
        "synthetic-calendar-refresh",
    ))));
    let provider = synthetic_provider(&server, browser_sender, credentials);
    provider
        .restore_account()
        .await
        .expect("synthetic account restores")
        .expect("synthetic account is connected");

    let event = provider
        .create_event(&creation)
        .await
        .expect("delegated Graph event creation succeeds");
    assert_eq!(event.reference(), &view);
    assert_eq!(event.subject(), Some("Synthetic planning meeting"));
    assert_eq!(event.time(), creation.draft().time());
    assert!(matches!(event.time(), CalendarOccurrenceTime::Timed { .. }));
    token_refresh.assert_async().await;
    profile_request.assert_async().await;
    event_request.assert_async().await;
}

#[tokio::test]
async fn create_event_rejects_a_view_for_another_signed_in_account_before_posting() {
    let server = MockServer::start_async().await;
    let (browser_sender, _browser_receiver) = mpsc::unbounded_channel();
    let credentials = Arc::new(MemoryRefreshTokenStore::new(Some(refresh_token(
        "synthetic-calendar-refresh",
    ))));
    let provider = synthetic_provider(&server, browser_sender, credentials);
    *provider.config.session.lock().await = Some(ProviderSession {
        account_id: AccountId::try_from("signed-in-calendar-account".to_owned())
            .expect("synthetic signed-in account ID is valid"),
        access_token: Some(access_token("synthetic-calendar-access")),
    });
    let date =
        Date::from_calendar_date(2030, Month::January, 8).expect("synthetic event date is valid");
    let start_date =
        Date::from_calendar_date(2030, Month::January, 7).expect("synthetic week start is valid");
    let end_date =
        Date::from_calendar_date(2030, Month::January, 14).expect("synthetic week end is valid");
    let account = AccountId::try_from("other-calendar-account".to_owned())
        .expect("synthetic foreign account ID is valid");
    let view = CalendarViewReference::new(
        account,
        CalendarViewRange::new(
            start_date.midnight().assume_utc(),
            end_date.midnight().assume_utc(),
            start_date,
            end_date,
        )
        .expect("synthetic calendar view is valid"),
    );
    let draft = CalendarEventDraft::timed(
        "Synthetic planning meeting".to_owned(),
        PrimitiveDateTime::new(
            date,
            Time::from_hms(9, 30, 0).expect("synthetic local start is valid"),
        ),
        PrimitiveDateTime::new(
            date,
            Time::from_hms(10, 0, 0).expect("synthetic local end is valid"),
        ),
        "Etc/UTC"
            .to_owned()
            .try_into()
            .expect("synthetic source zone is valid"),
        date.with_hms(9, 30, 0)
            .expect("synthetic UTC start is valid")
            .assume_utc(),
        date.with_hms(10, 0, 0)
            .expect("synthetic UTC end is valid")
            .assume_utc(),
    )
    .expect("synthetic event draft is valid");
    let creation = CalendarEventCreation::try_new(view, draft)
        .expect("synthetic event endpoints fit the selected week");

    assert_eq!(
        provider.create_event(&creation).await,
        Err(einfach_mail_application::CalendarOperationError::AccountMismatch)
    );
}
