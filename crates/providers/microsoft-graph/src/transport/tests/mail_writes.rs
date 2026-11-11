

#[tokio::test]
async fn throttled_read_state_patch_is_not_retried() {
    let server = MockServer::start_async().await;
    let throttled = server
        .mock_async(|when, then| {
            when.method(PATCH)
                .path("/v1.0/me/messages/synthetic-message");
            then.status(429).header("retry-after", "0");
        })
        .await;
    let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");
    let result = transport
        .handle()
        .patch_json(
            GraphPatchOperation::MessageUpdate,
            graph_url(&http_url(&server, "/v1.0/me/messages/synthetic-message")),
            synthetic_access_token(),
            Arc::from(&b"{\"isRead\":true}"[..]),
        )
        .await;

    assert_eq!(
        result,
        Err(GraphError::RateLimited {
            retry_after: Some(Duration::ZERO),
        })
    );
    throttled.assert_calls_async(1).await;
}

#[tokio::test]
async fn message_read_state_patch_uses_one_attempt_and_requires_exact_confirmation() {
    for status in [200, 201, 202, 204, 302, 503] {
        let server = MockServer::start_async().await;
        let patch = server
            .mock_async(move |when, then| {
                when.method(PATCH)
                    .path("/v1.0/me/messages/synthetic-message")
                    .header("authorization", "Bearer synthetic-access-token")
                    .header("content-type", "application/json")
                    .header("prefer", "IdType=\"ImmutableId\"")
                    .body(r#"{"isRead":true}"#);
                then.status(status).body(r#"{"isRead":true}"#);
            })
            .await;
        let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");

        let result = transport
            .handle()
            .patch_json(
                GraphPatchOperation::MessageUpdate,
                graph_url(&http_url(&server, "/v1.0/me/messages/synthetic-message")),
                synthetic_access_token(),
                Arc::from(&b"{\"isRead\":true}"[..]),
            )
            .await;

        let expected = if status == 200 {
            Ok(br#"{"isRead":true}"#.to_vec())
        } else {
            Err(GraphError::MessageUpdateOutcomeUnknown)
        };
        assert_eq!(result, expected, "PATCH response status {status}");
        patch.assert_calls_async(1).await;
    }
}

#[tokio::test]
async fn message_move_post_uses_immutable_ids_and_accepts_created_once() {
    let server = MockServer::start_async().await;
    let post = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/v1.0/me/messages/synthetic-message/move")
                .header("authorization", "Bearer synthetic-access-token")
                .header("content-type", "application/json")
                .header("prefer", "IdType=\"ImmutableId\"")
                .body(r#"{"destinationId":"archive"}"#);
            then.status(201).body("synthetic move response");
        })
        .await;
    let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");

    let response = transport
        .handle()
        .post_json(
            GraphPostJsonOperation::MessageMove,
            graph_url(&http_url(
                &server,
                "/v1.0/me/messages/synthetic-message/move",
            )),
            synthetic_access_token(),
            br#"{"destinationId":"archive"}"#.to_vec(),
        )
        .await
        .expect("201 returns the move confirmation body");

    assert_eq!(response, b"synthetic move response");
    post.assert_calls_async(1).await;
}

#[tokio::test]
async fn message_move_post_never_retries_uncertain_statuses() {
    for status in [200, 503] {
        let server = MockServer::start_async().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let responder_calls = Arc::clone(&calls);
        let post = server
            .mock_async(move |when, then| {
                when.method(POST)
                    .path("/v1.0/me/messages/synthetic-message/move");
                then.respond_with(move |_request: &HttpMockRequest| {
                    responder_calls.fetch_add(1, Ordering::SeqCst);
                    HttpMockResponse::builder()
                        .status(status)
                        .body("synthetic uncertain response")
                        .build()
                });
            })
            .await;
        let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");

        let result = transport
            .handle()
            .post_json(
                GraphPostJsonOperation::MessageMove,
                graph_url(&http_url(
                    &server,
                    "/v1.0/me/messages/synthetic-message/move",
                )),
                synthetic_access_token(),
                br#"{"destinationId":"archive"}"#.to_vec(),
            )
            .await;

        assert_eq!(result, Err(GraphError::MessageMoveOutcomeUnknown));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        post.assert_calls_async(1).await;
    }
}

#[tokio::test]
async fn message_read_state_patch_does_not_retry_throttle_responses() {
    let server = MockServer::start_async().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let responder_calls = Arc::clone(&calls);
    let patch = server
        .mock_async(move |when, then| {
            when.method(PATCH)
                .path("/v1.0/me/messages/synthetic-message");
            then.respond_with(move |_request: &HttpMockRequest| {
                responder_calls.fetch_add(1, Ordering::SeqCst);
                HttpMockResponse::builder()
                    .status(429)
                    .header("Retry-After", "0")
                    .body("synthetic throttle response")
                    .build()
            });
        })
        .await;
    let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");

    let result = transport
        .handle()
        .patch_json(
            GraphPatchOperation::MessageUpdate,
            graph_url(&http_url(&server, "/v1.0/me/messages/synthetic-message")),
            synthetic_access_token(),
            Arc::from(&b"{\"isRead\":true}"[..]),
        )
        .await;

    assert!(matches!(result, Err(GraphError::RateLimited { .. })));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    patch.assert_calls_async(1).await;
}

#[tokio::test]
async fn does_not_retry_a_send_throttle() {
    for (header, expected) in [
        (Some("0"), Some(Duration::ZERO)),
        (None, None),
        (Some("invalid"), None),
    ] {
        let server = MockServer::start_async().await;
        let header = header.map(str::to_owned);
        let throttle = server
            .mock_async(move |when, then| {
                when.method(POST).path("/send");
                then.respond_with(move |_| {
                    let mut response = HttpMockResponse::builder().status(429);
                    if let Some(header) = &header {
                        response = response.header("Retry-After", header);
                    }
                    response.build()
                });
            })
            .await;
        let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");

        let result = transport
            .handle()
            .post_mail(
                graph_url(&http_url(&server, "/send")),
                synthetic_access_token(),
                b"{}".to_vec(),
            )
            .await;

        assert_eq!(
            result,
            Err(GraphError::RateLimited {
                retry_after: expected
            })
        );
        throttle.assert_calls_async(1).await;
    }
}

#[tokio::test]
async fn classifies_send_statuses_without_treating_other_success_codes_as_accepted() {
    for (status, expected) in [
        (200, Err(GraphError::SendOutcomeUnknown)),
        (202, Ok(())),
        (400, Err(GraphError::SendRejected)),
        (401, Err(GraphError::AccessTokenRejected)),
        (403, Err(GraphError::AccessDenied)),
        (408, Err(GraphError::SendOutcomeUnknown)),
        (500, Err(GraphError::SendOutcomeUnknown)),
    ] {
        let server = MockServer::start_async().await;
        let response = server
            .mock_async(|when, then| {
                when.method(POST).path("/send");
                then.status(status);
            })
            .await;
        let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");

        let result = transport
            .handle()
            .post_mail(
                graph_url(&http_url(&server, "/send")),
                synthetic_access_token(),
                b"{}".to_vec(),
            )
            .await;

        assert_eq!(
            result, expected,
            "unexpected classification for HTTP {status}"
        );
        response.assert_calls_async(1).await;
    }
}

#[tokio::test]
async fn does_not_follow_a_send_redirect() {
    let server = MockServer::start_async().await;
    let redirect = server
        .mock_async(|when, then| {
            when.method(POST).path("/send");
            then.status(302).header("Location", "/redirected");
        })
        .await;
    let redirect_target = server
        .mock_async(|when, then| {
            when.method(GET).path("/redirected");
            then.status(202);
        })
        .await;
    let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");

    let result = transport
        .handle()
        .post_mail(
            graph_url(&http_url(&server, "/send")),
            synthetic_access_token(),
            b"{}".to_vec(),
        )
        .await;

    assert_eq!(result, Err(GraphError::SendOutcomeUnknown));
    redirect.assert_calls_async(1).await;
    redirect_target.assert_calls_async(0).await;
}

#[tokio::test]
async fn connection_closed_after_a_send_request_has_unknown_outcome() {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("synthetic loopback listener binds");
    let address = listener
        .local_addr()
        .expect("synthetic loopback listener has an address");
    let accept_and_close = tokio::spawn(async move {
        let (mut stream, _) = listener
            .accept()
            .await
            .expect("synthetic send request connects");
        let mut request = [0_u8; 2048];
        let received = stream
            .read(&mut request)
            .await
            .expect("synthetic send request bytes are readable");
        assert!(received > 0);
        drop(stream);
    });
    let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");

    let result = transport
        .handle()
        .post_mail(
            graph_url(&format!("http://{address}/send")),
            synthetic_access_token(),
            b"{}".to_vec(),
        )
        .await;

    assert_eq!(result, Err(GraphError::SendOutcomeUnknown));
    accept_and_close
        .await
        .expect("synthetic server closes after receiving the request");
}

#[tokio::test]
async fn post_throttle_response_is_not_retried() {
    let server = MockServer::start_async().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let responder_calls = Arc::clone(&calls);
    let post = server
        .mock_async(move |when, then| {
            when.method(POST).path("/v1.0/me/calendar/events");
            then.respond_with(move |_request: &HttpMockRequest| {
                responder_calls.fetch_add(1, Ordering::SeqCst);
                HttpMockResponse::builder()
                    .status(429)
                    .header("Retry-After", "0")
                    .body("synthetic throttle response")
                    .build()
            });
        })
        .await;
    let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");

    let result = transport
        .handle()
        .post_json(
            GraphPostJsonOperation::CalendarEventCreate,
            graph_url(&http_url(&server, "/v1.0/me/calendar/events")),
            synthetic_access_token(),
            b"{}".to_vec(),
        )
        .await;

    assert_eq!(result, Err(GraphError::CalendarEventCreationOutcomeUnknown));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    post.assert_calls_async(1).await;
}