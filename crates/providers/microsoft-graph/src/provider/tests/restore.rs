

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
    let (provider, account) = connect_with_secret_service(&server, Arc::clone(&credentials)).await;
    let account_id = account.profile().id().clone();

    assert_eq!(account_id.as_str(), "synthetic-account");
    assert_saved_refresh_token(&credentials, &account_id, "synthetic-connect-refresh");
    drop(provider);

    let (provider, restored_profile) =
        restore_with_secret_service(&server, Arc::clone(&credentials)).await;

    assert_eq!(restored_profile.profile().id(), &account_id);
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
    let account = restored.expect("saved refresh token identifies an account");
    assert_eq!(account.provider_kind(), AccountProviderKind::MicrosoftGraph);
    let profile = account.profile();

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
