

#[test]
fn rejects_ambiguous_calendar_reset_locations() {
    let mut headers = HeaderMap::new();
    headers.append(
        super::LOCATION,
        "https://graph.microsoft.com/v1.0/me/calendarView/delta"
            .parse()
            .expect("synthetic Location is a valid header"),
    );
    headers.append(
        super::LOCATION,
        "https://example.test/v1.0/me/calendarView/delta"
            .parse()
            .expect("synthetic Location is a valid header"),
    );

    assert_eq!(
        calendar_reset_location(&headers),
        Err(GraphError::UnsafeLink)
    );
}

#[test]
fn marks_the_bearer_header_as_sensitive() {
    let header = authorization_header(&synthetic_access_token())
        .expect("synthetic bearer token forms a header");

    assert!(header.is_sensitive());
}

#[test]
fn caps_response_chunks_at_the_body_limit() {
    let mut body = vec![b'x'; super::MAX_RESPONSE_BODY_BYTES];

    assert_eq!(
        append_response_chunk(&mut body, b"x"),
        Err(GraphError::ResponseTooLarge)
    );
    assert_eq!(body.len(), super::MAX_RESPONSE_BODY_BYTES);
}

#[tokio::test]
async fn rejects_an_oversized_fixture_response() {
    let server = MockServer::start_async().await;
    let oversized_body = String::from_utf8(vec![b'x'; super::MAX_RESPONSE_BODY_BYTES + 1])
        .expect("synthetic response body is valid UTF-8");
    let response = server
        .mock_async(|when, then| {
            when.method(GET).path("/oversized");
            then.status(200).body(oversized_body);
        })
        .await;
    let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");

    let result = transport
        .handle()
        .get(
            GraphReadOperation::Profile,
            graph_url(&http_url(&server, "/oversized")),
            synthetic_access_token(),
        )
        .await;

    assert_eq!(result, Err(GraphError::ResponseTooLarge));
    response.assert_async().await;
}

#[tokio::test]
async fn returns_delta_reset_location_only_for_message_delta_requests() {
    let server = MockServer::start_async().await;
    let location = format!(
        "{}/v1.0/me/mailFolders/folder/messages/delta?$deltatoken=",
        http_base_url(&server)
    );
    let response = server
        .mock_async(move |when, then| {
            when.method(GET).path("/delta");
            then.status(410).header("Location", location);
        })
        .await;
    let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");
    let handle = transport.handle();
    let url = graph_url(&http_url(&server, "/delta"));

    let reset = handle
        .get_message_delta(url.clone(), synthetic_access_token())
        .await
        .expect("message delta reset is returned to the Graph adapter");
    let other_operation = handle
        .get(GraphReadOperation::Profile, url, synthetic_access_token())
        .await;

    assert_eq!(
        reset,
        GraphResponse::MessageDeltaReset(Some(format!(
            "{}/v1.0/me/mailFolders/folder/messages/delta?$deltatoken=",
            http_base_url(&server)
        )))
    );
    assert_eq!(other_operation, Err(GraphError::Transport));
    response.assert_calls_async(2).await;
}

#[tokio::test]
async fn classifies_calendar_410_resets_only_for_continuation_requests() {
    let server = MockServer::start_async().await;
    let location = format!(
        "{}/v1.0/me/calendarView/delta?startDateTime=2030-01-07T00%3A00%3A00Z&endDateTime=2030-01-14T00%3A00%3A00Z&$deltatoken=",
        http_base_url(&server)
    );
    let mock_location = location.clone();
    let response = server
        .mock_async(move |when, then| {
            when.method(GET)
                .path("/delta")
                .header("content-type", "application/json")
                .header("prefer", "IdType=\"ImmutableId\"");
            then.status(410).header("Location", mock_location.clone());
        })
        .await;
    let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");
    let handle = transport.handle();
    let url = graph_url(&http_url(&server, "/delta"));

    let reset = handle
        .get_calendar_delta(
            GraphCalendarDeltaOperation::Continuation,
            url.clone(),
            synthetic_access_token(),
        )
        .await
        .expect("continuation reset response is returned to the Graph adapter");
    let initial = handle
        .get_calendar_delta(
            GraphCalendarDeltaOperation::Initial,
            url,
            synthetic_access_token(),
        )
        .await;

    assert_eq!(
        reset,
        GraphResponse::CalendarViewDeltaReset {
            location: Some(location.clone()),
            response_bytes: location.len(),
        }
    );
    assert_eq!(initial, Err(GraphError::Transport));
    response.assert_calls_async(2).await;
}

#[tokio::test]
async fn maps_only_the_exact_sync_state_not_found_error_code() {
    let server = MockServer::start_async().await;
    let expiration_body = r#"{"error":{"code":"syncStateNotFound"}}"#;
    let unrelated_body = r#"{"error":{"code":"InvalidAuthenticationToken"}}"#;
    let expiration = server
        .mock_async(|when, then| {
            when.method(GET).path("/expired");
            then.status(400).body(expiration_body);
        })
        .await;
    let unrelated = server
        .mock_async(|when, then| {
            when.method(GET).path("/unrelated");
            then.status(400).body(unrelated_body);
        })
        .await;
    let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");
    let handle = transport.handle();

    let reset = handle
        .get_calendar_delta(
            GraphCalendarDeltaOperation::Continuation,
            graph_url(&http_url(&server, "/expired")),
            synthetic_access_token(),
        )
        .await
        .expect("the exact Graph expiration code is returned as a reset");
    let error = handle
        .get_calendar_delta(
            GraphCalendarDeltaOperation::Continuation,
            graph_url(&http_url(&server, "/unrelated")),
            synthetic_access_token(),
        )
        .await;

    assert_eq!(
        reset,
        GraphResponse::CalendarViewDeltaReset {
            location: None,
            response_bytes: expiration_body.len(),
        }
    );
    assert_eq!(error, Err(GraphError::Transport));
    expiration.assert_async().await;
    unrelated.assert_async().await;
}

#[test]
fn parses_retry_after_seconds_and_dates_without_early_retries() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_798_000_000);

    assert_eq!(
        parse_retry_after("17", now),
        Some(RetryAfter::Delay(Duration::from_secs(17)))
    );
    assert_eq!(
        parse_retry_after("99999999999999999999", now),
        Some(RetryAfter::Unrepresentable)
    );
    assert_eq!(fallback_delay(0), Duration::from_secs(1));
    assert_eq!(fallback_delay(10), Duration::from_secs(60));
    let date = httpdate::fmt_http_date(now + Duration::from_secs(31));
    assert_eq!(
        parse_retry_after(&date, now),
        Some(RetryAfter::Delay(Duration::from_secs(31)))
    );
    assert_eq!(parse_retry_after("invalid", now), None);
    assert_eq!(retry_after_hint(None, now), None);
}

#[tokio::test]
async fn dropping_callers_cancels_active_requests_and_frees_worker_capacity() {
    let server = MockServer::start_async().await;
    let slow_response = server
        .mock_async(|when, then| {
            when.method(GET).path("/slow");
            then.status(200)
                .delay(Duration::from_secs(20))
                .body("synthetic delayed response");
        })
        .await;
    let fast_response = server
        .mock_async(|when, then| {
            when.method(GET).path("/probe");
            then.status(200).body("synthetic fast response");
        })
        .await;
    let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");
    let handle = transport.handle();
    let slow_url = graph_url(&http_url(&server, "/slow"));
    let requests = (0..super::MAX_ACTIVE_REQUESTS)
        .map(|_| {
            let handle = handle.clone();
            let slow_url = slow_url.clone();
            let token = synthetic_access_token();
            tokio::spawn(async move {
                handle
                    .get(GraphReadOperation::Profile, slow_url, token)
                    .await
            })
        })
        .collect::<Vec<_>>();
    wait_for_mock_calls(&slow_response, super::MAX_ACTIVE_REQUESTS).await;

    for request in requests {
        request.abort();
        assert!(request.await.is_err(), "caller task was cancelled");
    }

    let response = timeout(
        REQUEST_WAIT,
        handle.get(
            GraphReadOperation::Profile,
            graph_url(&http_url(&server, "/probe")),
            synthetic_access_token(),
        ),
    )
    .await
    .expect("cancelled requests free worker capacity")
    .expect("synthetic probe request succeeds");

    assert_eq!(response, b"synthetic fast response");
    fast_response.assert_async().await;
}

#[tokio::test]
async fn dropping_the_provider_joins_the_worker_with_active_requests() {
    let server = MockServer::start_async().await;
    let slow_response = server
        .mock_async(|when, then| {
            when.method(GET).path("/slow");
            then.status(200)
                .delay(Duration::from_secs(20))
                .body("synthetic delayed response");
        })
        .await;
    let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");
    let handle = transport.handle();
    let slow_url = graph_url(&http_url(&server, "/slow"));
    let request = tokio::spawn(async move {
        handle
            .get(
                GraphReadOperation::Profile,
                slow_url,
                synthetic_access_token(),
            )
            .await
    });
    wait_for_mock_calls(&slow_response, 1).await;
    let started = Instant::now();

    drop(transport);

    assert!(started.elapsed() < REQUEST_WAIT);
    assert_eq!(
        request.await.expect("request caller exits after shutdown"),
        Err(GraphError::WorkerClosed)
    );
}

#[tokio::test]
async fn worker_closure_after_read_state_patch_dispatch_reports_unknown() {
    let server = MockServer::start_async().await;
    let patch = server
        .mock_async(|when, then| {
            when.method(PATCH)
                .path("/v1.0/me/messages/synthetic-message");
            then.status(200)
                .delay(Duration::from_secs(20))
                .body(r#"{"isRead":true}"#);
        })
        .await;
    let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");
    let handle = transport.handle();
    let url = graph_url(&http_url(&server, "/v1.0/me/messages/synthetic-message"));
    let request = tokio::spawn(async move {
        handle
            .patch_json(
                GraphPatchOperation::MessageUpdate,
                url,
                synthetic_access_token(),
                Arc::from(&b"{\"isRead\":true}"[..]),
            )
            .await
    });
    wait_for_mock_calls(&patch, 1).await;

    drop(transport);

    assert_eq!(
        request
            .await
            .expect("PATCH caller exits after worker shutdown"),
        Err(GraphError::MessageUpdateOutcomeUnknown)
    );
}

#[tokio::test]
async fn system_proxy_routes_the_production_transport_through_http_proxy() {
    if std::env::var_os(SYSTEM_PROXY_CHILD).is_some() {
        assert_proxy_child_request().await;
        return;
    }

    let proxy = MockServer::start_async().await;
    let routed_request = proxy
        .mock_async(|when, then| {
            when.any_request();
            then.status(200).body("synthetic proxy response");
        })
        .await;
    let proxy_url = http_base_url(&proxy);
    let child_output =
        Command::new(std::env::current_exe().expect("current test executable is available"))
            .args([
                "--exact",
                "transport::tests::system_proxy_routes_the_production_transport_through_http_proxy",
            ])
            .env(SYSTEM_PROXY_CHILD, "1")
            .env("HTTP_PROXY", &proxy_url)
            .env("http_proxy", &proxy_url)
            .env("NO_PROXY", "")
            .env("no_proxy", "")
            .env_remove("ALL_PROXY")
            .env_remove("all_proxy")
            .env_remove("HTTPS_PROXY")
            .env_remove("https_proxy")
            .output()
            .expect("system-proxy test child starts");
    let stdout = String::from_utf8_lossy(&child_output.stdout);
    assert!(
        child_output.status.success() && stdout.contains("running 1 test"),
        "system-proxy child failed: stdout: {stdout}; stderr: {}",
        String::from_utf8_lossy(&child_output.stderr)
    );
    routed_request.assert_calls_async(1).await;
}

#[tokio::test]
async fn retries_a_synthetic_graph_throttle_response() {
    let server = MockServer::start_async().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let responder_calls = Arc::clone(&calls);
    let throttle = server
        .mock_async(move |when, then| {
            when.method(GET).path("/throttled");
            then.respond_with(move |_request: &HttpMockRequest| {
                if responder_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    HttpMockResponse::builder()
                        .status(429)
                        .header("Retry-After", "0")
                        .body("synthetic throttle response")
                        .build()
                } else {
                    HttpMockResponse::builder()
                        .status(200)
                        .body("synthetic retry response")
                        .build()
                }
            });
        })
        .await;
    let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");

    let response = transport
        .handle()
        .get(
            GraphReadOperation::Folders,
            graph_url(&http_url(&server, "/throttled")),
            synthetic_access_token(),
        )
        .await
        .expect("the production worker retries a 429 with a safe delay");

    assert_eq!(response, b"synthetic retry response");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    throttle.assert_calls_async(2).await;
}

#[tokio::test]
async fn http_errors_do_not_retain_the_request_url_or_response_body() {
    let server = MockServer::start_async().await;
    let response = server
        .mock_async(|when, then| {
            when.method(GET).path("/failure");
            then.status(500).body("synthetic-response-secret");
        })
        .await;
    let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");
    let url = format!(
        "{}?opaque=synthetic-url-secret",
        http_url(&server, "/failure")
    );

    let error = transport
        .handle()
        .get(
            GraphReadOperation::Profile,
            graph_url(&url),
            synthetic_access_token(),
        )
        .await
        .expect_err("synthetic server error is rejected");
    let rendered = format!("{error:?}");

    assert_eq!(error, GraphError::Transport);
    assert!(!rendered.contains("synthetic-access-token"));
    assert!(!rendered.contains("synthetic-url-secret"));
    assert!(!rendered.contains("synthetic-response-secret"));
    response.assert_async().await;
}

#[test]
fn rate_limit_stops_after_the_retry_budget_or_long_delay() {
    let decision = retry_decision_for(RetryAfter::Delay(Duration::from_secs(120)), 0);

    assert!(
        matches!(decision, RetryDecision::Stop(GraphError::RateLimited { retry_after: Some(delay) }) if delay == Duration::from_secs(120))
    );
}