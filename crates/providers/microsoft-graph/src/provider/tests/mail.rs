

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
                .query_param("$select", "id,displayName,isHidden,unreadItemCount")
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
        .well_known_folder(&account, MailFolderKind::DeletedItems)
        .await
        .expect("synthetic Deleted Items folder resolves");
    let other_account =
        AccountId::try_from("other-account".to_owned()).expect("synthetic account ID is valid");

    assert_eq!(folder.account_id(), &account);
    assert_eq!(folder.folder_id().as_str(), "deleted-folder");
    assert_eq!(
        provider
            .well_known_folder(&other_account, MailFolderKind::DeletedItems)
            .await,
        Err(MailOperationError::AccountMismatch)
    );
    lookup.assert_calls_async(1).await;
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

    let account = provider
        .restore_account()
        .await
        .expect("synthetic account restores")
        .expect("synthetic account is present");
    let profile = account.profile();
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
