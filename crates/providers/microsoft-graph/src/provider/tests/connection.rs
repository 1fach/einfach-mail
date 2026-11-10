

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

    let account = connect_task
        .await
        .expect("provider task completes")
        .expect("synthetic account connection succeeds");
    assert_eq!(account.provider_kind(), AccountProviderKind::MicrosoftGraph);
    let profile = account.profile();
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

    let connected_account = connection
        .await
        .expect("connection workflow completes")
        .expect("commit wins the cancellation race");
    assert_eq!(connected_account.profile().id().as_str(), "account-123");
    assert_eq!(
        credentials.token_value().as_deref(),
        Some("synthetic-connect-refresh")
    );

    let (restored_browser_sender, _) = mpsc::unbounded_channel();
    let restored_provider =
        synthetic_provider(&server, restored_browser_sender, credentials.clone());
    let restored_workflow = AccountWorkflow::new(restored_provider);
    let restored_account = restored_workflow
        .restore_account()
        .await
        .expect("new workflow restores the committed account")
        .expect("committed account is present");
    assert_eq!(
        restored_account.profile().id(),
        connected_account.profile().id()
    );
    assert_eq!(
        credentials.token_value().as_deref(),
        Some("synthetic-recovered-refresh")
    );
    token_exchange.assert_async().await;
    profile_request.assert_async().await;
    restored_token.assert_async().await;
    restored_profile_request.assert_async().await;
}
