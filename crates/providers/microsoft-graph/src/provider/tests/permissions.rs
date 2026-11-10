

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
    assert_eq!(profile.profile().id(), &expected_account);
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
    let expected_account_id = active_profile.profile().id().clone();

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
    assert_eq!(
        renewed_profile.profile().id(),
        active_profile.profile().id()
    );
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
    assert_eq!(
        restored_profile.profile().id(),
        renewed_profile.profile().id()
    );
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

    let expected_account_id = active_profile.profile().id().clone();
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
    assert_eq!(session.account_id, *active_profile.profile().id());
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
