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

fn graph_url(value: &str) -> GraphUrl {
    GraphUrl::fixture(Url::parse(value).expect("synthetic Graph URL is valid"))
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

include!("tests/worker.rs");
include!("tests/mail_writes.rs");
include!("tests/calendar_writes.rs");
