

#[tokio::test]
async fn post_sends_json_and_uses_immutable_calendar_event_ids() {
    let server = MockServer::start_async().await;
    let post = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/v1.0/me/calendar/events")
                .header("authorization", "Bearer synthetic-access-token")
                .header("content-type", "application/json")
                .header("prefer", "IdType=\"ImmutableId\"")
                .body(r#"{"subject":"Planning","transactionId":"synthetic-id"}"#);
            then.status(201).body("synthetic created event");
        })
        .await;
    let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");

    let response = transport
        .handle()
        .post_json(
            GraphPostJsonOperation::CalendarEventCreate,
            graph_url(&http_url(&server, "/v1.0/me/calendar/events")),
            synthetic_access_token(),
            b"{\"subject\":\"Planning\",\"transactionId\":\"synthetic-id\"}".to_vec(),
        )
        .await
        .expect("synthetic event POST succeeds");

    assert_eq!(response, b"synthetic created event");
    post.assert_async().await;
}

#[tokio::test]
async fn post_treats_a_success_status_that_does_not_confirm_creation_as_unknown() {
    let server = MockServer::start_async().await;
    let post = server
        .mock_async(|when, then| {
            when.method(POST).path("/v1.0/me/calendar/events");
            then.status(200).body("synthetic non-creation response");
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
    post.assert_async().await;
}

#[tokio::test]
async fn calendar_event_update_server_failure_is_unknown_and_not_retried() {
    let server = MockServer::start_async().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let responder_calls = Arc::clone(&calls);
    let patch = server
        .mock_async(move |when, then| {
            when.method(PATCH)
                .path("/v1.0/me/events/synthetic-event")
                .header("prefer", "IdType=\"ImmutableId\"");
            then.respond_with(move |_request: &HttpMockRequest| {
                responder_calls.fetch_add(1, Ordering::SeqCst);
                HttpMockResponse::builder()
                    .status(503)
                    .body("synthetic uncertain event update")
                    .build()
            });
        })
        .await;
    let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");

    let result = transport
        .handle()
        .patch_json(
            GraphPatchOperation::CalendarEventUpdate,
            graph_url(&http_url(&server, "/v1.0/me/events/synthetic-event")),
            synthetic_access_token(),
            Arc::from(&b"{\"subject\":\"Updated\"}"[..]),
        )
        .await;

    assert_eq!(result, Err(GraphError::CalendarEventUpdateOutcomeUnknown));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    patch.assert_calls_async(1).await;
}

#[tokio::test]
async fn calendar_event_update_redirect_is_unknown_and_not_retried() {
    let server = MockServer::start_async().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let responder_calls = Arc::clone(&calls);
    let patch = server
        .mock_async(move |when, then| {
            when.method(PATCH)
                .path("/v1.0/me/events/synthetic-event")
                .header("prefer", "IdType=\"ImmutableId\"");
            then.respond_with(move |_request: &HttpMockRequest| {
                responder_calls.fetch_add(1, Ordering::SeqCst);
                HttpMockResponse::builder()
                    .status(302)
                    .header("Location", "https://example.test/redirect")
                    .body("synthetic redirected event update")
                    .build()
            });
        })
        .await;
    let redirect_target = server
        .mock_async(|when, then| {
            when.method(GET).path("/redirect");
            then.status(200);
        })
        .await;
    let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");

    let result = transport
        .handle()
        .patch_json(
            GraphPatchOperation::CalendarEventUpdate,
            graph_url(&http_url(&server, "/v1.0/me/events/synthetic-event")),
            synthetic_access_token(),
            Arc::from(&b"{\"subject\":\"Updated\"}"[..]),
        )
        .await;

    assert_eq!(result, Err(GraphError::CalendarEventUpdateOutcomeUnknown));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    patch.assert_calls_async(1).await;
    redirect_target.assert_calls_async(0).await;
}

#[tokio::test]
async fn calendar_event_update_not_found_reports_a_changed_event_without_retrying() {
    let server = MockServer::start_async().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let responder_calls = Arc::clone(&calls);
    let patch = server
        .mock_async(move |when, then| {
            when.method(PATCH)
                .path("/v1.0/me/events/synthetic-event")
                .header("prefer", "IdType=\"ImmutableId\"");
            then.respond_with(move |_request: &HttpMockRequest| {
                responder_calls.fetch_add(1, Ordering::SeqCst);
                HttpMockResponse::builder().status(404).build()
            });
        })
        .await;
    let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");

    let result = transport
        .handle()
        .patch_json(
            GraphPatchOperation::CalendarEventUpdate,
            graph_url(&http_url(&server, "/v1.0/me/events/synthetic-event")),
            synthetic_access_token(),
            Arc::from(&b"{\"subject\":\"Updated\"}"[..]),
        )
        .await;

    assert_eq!(result, Err(GraphError::CalendarEventChanged));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    patch.assert_calls_async(1).await;
}

#[tokio::test]
async fn calendar_event_delete_sends_the_etag_without_a_body() {
    let server = MockServer::start_async().await;
    let delete = server
        .mock_async(|when, then| {
            when.method(DELETE)
                .path("/v1.0/me/events/synthetic-event")
                .header("authorization", "Bearer synthetic-access-token")
                .header("if-match", "W/\"synthetic-etag\"")
                .header("prefer", "IdType=\"ImmutableId\"")
                .header_missing("content-type")
                .body("");
            then.status(204);
        })
        .await;
    let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");

    transport
        .handle()
        .delete_calendar_event(
            graph_url(&http_url(&server, "/v1.0/me/events/synthetic-event")),
            synthetic_access_token(),
            "W/\"synthetic-etag\"",
        )
        .await
        .expect("Graph confirms the empty 204 response");

    delete.assert_calls_async(1).await;
}

#[tokio::test]
async fn calendar_event_delete_classifies_responses_without_retries() {
    for (status, expected) in [
        (204, Ok(())),
        (400, Err(GraphError::CalendarEventDeletionRejected)),
        (401, Err(GraphError::AccessTokenRejected)),
        (403, Err(GraphError::AccessDenied)),
        (404, Err(GraphError::CalendarEventChanged)),
        (408, Err(GraphError::CalendarEventDeletionOutcomeUnknown)),
        (412, Err(GraphError::CalendarEventChanged)),
        (
            429,
            Err(GraphError::RateLimited {
                retry_after: Some(Duration::from_secs(1)),
            }),
        ),
        (500, Err(GraphError::CalendarEventDeletionOutcomeUnknown)),
        (302, Err(GraphError::CalendarEventDeletionOutcomeUnknown)),
        (202, Err(GraphError::CalendarEventDeletionOutcomeUnknown)),
    ] {
        let server = MockServer::start_async().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let responder_calls = Arc::clone(&calls);
        let response = server
            .mock_async(move |when, then| {
                when.method(DELETE).path("/v1.0/me/events/synthetic-event");
                then.respond_with(move |_request: &HttpMockRequest| {
                    responder_calls.fetch_add(1, Ordering::SeqCst);
                    HttpMockResponse::builder().status(status).build()
                });
            })
            .await;
        let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");

        let result = transport
            .handle()
            .delete_calendar_event(
                graph_url(&http_url(&server, "/v1.0/me/events/synthetic-event")),
                synthetic_access_token(),
                "W/\"synthetic-etag\"",
            )
            .await;

        assert_eq!(
            result, expected,
            "unexpected classification for HTTP {status}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        response.assert_calls_async(1).await;
    }
}

#[tokio::test]
async fn calendar_event_delete_redirect_is_unknown_and_not_followed() {
    let server = MockServer::start_async().await;
    let delete = server
        .mock_async(|when, then| {
            when.method(DELETE).path("/v1.0/me/events/synthetic-event");
            then.status(302).header("Location", "/redirect");
        })
        .await;
    let redirect_target = server
        .mock_async(|when, then| {
            when.method(GET).path("/redirect");
            then.status(200);
        })
        .await;
    let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");

    assert_eq!(
        transport
            .handle()
            .delete_calendar_event(
                graph_url(&http_url(&server, "/v1.0/me/events/synthetic-event")),
                synthetic_access_token(),
                "W/\"synthetic-etag\"",
            )
            .await,
        Err(GraphError::CalendarEventDeletionOutcomeUnknown)
    );
    delete.assert_calls_async(1).await;
    redirect_target.assert_calls_async(0).await;
}

#[tokio::test]
async fn calendar_event_delete_rejects_invalid_etags_before_sending() {
    let server = MockServer::start_async().await;
    let delete = server
        .mock_async(|when, then| {
            when.method(DELETE).path("/v1.0/me/events/synthetic-event");
            then.status(204);
        })
        .await;
    let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");

    for etag in [
        "*",
        " *",
        "* ",
        "synthetic-etag",
        "\"unterminated",
        "W/\"unterminated",
        "\"synthetic-etag\" suffix",
    ] {
        assert_eq!(
            transport
                .handle()
                .delete_calendar_event(
                    graph_url(&http_url(&server, "/v1.0/me/events/synthetic-event")),
                    synthetic_access_token(),
                    etag,
                )
                .await,
            Err(GraphError::InvalidResponse),
            "invalid ETag {etag:?} must be rejected before sending"
        );
    }

    delete.assert_calls_async(0).await;
}

#[tokio::test]
async fn classifies_calendar_creation_responses_without_retrying() {
    for (status, expected) in [
        (201, Ok(())),
        (400, Err(GraphError::CalendarEventCreationRejected)),
        (401, Err(GraphError::AccessTokenRejected)),
        (403, Err(GraphError::AccessDenied)),
        (408, Err(GraphError::CalendarEventCreationOutcomeUnknown)),
        (409, Err(GraphError::CalendarEventCreationOutcomeUnknown)),
        (500, Err(GraphError::CalendarEventCreationOutcomeUnknown)),
        (302, Err(GraphError::CalendarEventCreationOutcomeUnknown)),
    ] {
        let server = MockServer::start_async().await;
        let response = server
            .mock_async(|when, then| {
                when.method(POST).path("/v1.0/me/calendar/events");
                then.status(status).body("{}");
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
            .await
            .map(|_| ());

        assert_eq!(
            result, expected,
            "unexpected classification for HTTP {status}"
        );
        response.assert_calls_async(1).await;
    }
}