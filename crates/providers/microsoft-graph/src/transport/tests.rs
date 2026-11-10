use std::{
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use httpmock::{
    HttpMockRequest, HttpMockResponse,
    Method::{DELETE, GET, PATCH, POST},
    Mock, MockServer,
};
use huskarl::{
    core::secrets::SecretString,
    token::{AccessToken, BearerAccessToken},
};
use reqwest::header::HeaderMap;
use tokio::{io::AsyncReadExt, net::TcpListener, time::timeout};
use url::Url;

use super::{
    GraphCalendarDeltaOperation, GraphPatchOperation, GraphPostJsonOperation, GraphReadOperation,
    GraphResponse, GraphTransport, RetryAfter, RetryDecision, append_response_chunk,
    authorization_header, calendar_reset_location, fallback_delay, parse_retry_after,
    retry_after_hint, retry_decision_for,
};
use crate::{
    error::GraphError,
    test_support::{http_base_url, http_url},
    transport::GraphUrl,
};

const SYSTEM_PROXY_CHILD: &str = "EINFACH_MAIL_GRAPH_PROVIDER_PROXY_TEST_CHILD";
const REQUEST_WAIT: Duration = Duration::from_secs(3);

fn synthetic_access_token() -> Arc<AccessToken> {
    Arc::new(AccessToken::Bearer(BearerAccessToken::new(
        SecretString::new("synthetic-access-token"),
        SystemTime::now(),
        Some(Duration::from_secs(3600)),
    )))
}

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

fn graph_url(value: &str) -> GraphUrl {
    GraphUrl::fixture(Url::parse(value).expect("synthetic Graph URL is valid"))
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

async fn assert_proxy_child_request() {
    let transport = GraphTransport::start().expect("system-proxy worker starts");
    let response = transport
        .handle()
        .get(
            GraphReadOperation::Profile,
            graph_url("http://proxy-check.example.test/v1.0/probe"),
            synthetic_access_token(),
        )
        .await
        .expect("synthetic request reaches configured proxy");

    assert_eq!(response, b"synthetic proxy response");
}

async fn wait_for_mock_calls(mock: &Mock<'_>, expected_calls: usize) {
    timeout(REQUEST_WAIT, async {
        while mock.calls_async().await < expected_calls {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("synthetic request reaches the fixture server");
}

#[test]
fn rate_limit_stops_after_the_retry_budget_or_long_delay() {
    let decision = retry_decision_for(RetryAfter::Delay(Duration::from_secs(120)), 0);

    assert!(
        matches!(decision, RetryDecision::Stop(GraphError::RateLimited { retry_after: Some(delay) }) if delay == Duration::from_secs(120))
    );
}
