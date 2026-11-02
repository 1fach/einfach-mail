use std::{
    future::Future,
    sync::{Arc, mpsc as sync_mpsc},
    thread::{self, JoinHandle},
    time::{Duration, SystemTime},
};

use huskarl::token::AccessToken;
use reqwest::{
    Client, Response, StatusCode,
    header::{AUTHORIZATION, HeaderValue, LOCATION, RETRY_AFTER},
};
use tokio::{
    runtime::{Builder, Runtime},
    sync::{mpsc, oneshot},
    task::JoinSet,
};

use crate::{error::GraphError, graph_url::GraphUrl, http_client};

const REQUEST_CHANNEL_CAPACITY: usize = 16;
const MAX_ACTIVE_REQUESTS: usize = 16;
const MAX_RESPONSE_BODY_BYTES: usize = 8 * 1024 * 1024;
const MAX_THROTTLE_RETRIES: u32 = 2;
const MAX_AUTOMATIC_RETRY_DELAY: Duration = Duration::from_secs(60);

#[derive(Clone, Copy)]
pub(super) enum GraphOperation {
    Profile,
    Folders,
    Messages,
    MessageDelta,
    MessageBody,
}

impl GraphOperation {
    fn uses_immutable_ids(self) -> bool {
        matches!(
            self,
            Self::Messages | Self::MessageDelta | Self::MessageBody
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RetryAfter {
    Delay(Duration),
    Unrepresentable,
}

struct GraphRequest {
    operation: GraphOperation,
    url: GraphUrl,
    token: Arc<AccessToken>,
}

struct GetCommand {
    request: GraphRequest,
    reply: oneshot::Sender<Result<GraphResponse, GraphError>>,
}

#[derive(Eq, PartialEq)]
pub(super) enum GraphResponse {
    Body(Vec<u8>),
    MessageDeltaReset(Option<String>),
}

impl std::fmt::Debug for GraphResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Body(_) => formatter.write_str("Body(<redacted>)"),
            Self::MessageDeltaReset(_) => formatter.write_str("MessageDeltaReset(<redacted>)"),
        }
    }
}

impl GraphResponse {
    fn into_body(self) -> Result<Vec<u8>, GraphError> {
        match self {
            Self::Body(body) => Ok(body),
            Self::MessageDeltaReset(_) => Err(GraphError::InvalidResponse),
        }
    }
}

#[derive(Clone)]
pub(super) struct GraphTransportHandle {
    commands: mpsc::Sender<GetCommand>,
}

impl GraphTransportHandle {
    pub(super) async fn get(
        &self,
        operation: GraphOperation,
        url: GraphUrl,
        token: Arc<AccessToken>,
    ) -> Result<Vec<u8>, GraphError> {
        self.request(operation, url, token).await?.into_body()
    }

    pub(super) async fn get_message_delta(
        &self,
        url: GraphUrl,
        token: Arc<AccessToken>,
    ) -> Result<GraphResponse, GraphError> {
        self.request(GraphOperation::MessageDelta, url, token).await
    }

    async fn request(
        &self,
        operation: GraphOperation,
        url: GraphUrl,
        token: Arc<AccessToken>,
    ) -> Result<GraphResponse, GraphError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(GetCommand {
                request: GraphRequest {
                    operation,
                    url,
                    token,
                },
                reply,
            })
            .await
            .map_err(|_| GraphError::WorkerClosed)?;
        response.await.map_err(|_| GraphError::WorkerClosed)?
    }
}

pub(super) struct GraphTransport {
    handle: GraphTransportHandle,
    shutdown: Option<oneshot::Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

impl GraphTransport {
    pub(super) fn start() -> Result<Self, GraphError> {
        WorkerStartup::new().launch(build_system_client)
    }

    #[cfg(test)]
    pub(super) fn start_for_fixtures() -> Result<Self, GraphError> {
        WorkerStartup::new().launch(build_fixture_client)
    }

    pub(super) fn handle(&self) -> GraphTransportHandle {
        self.handle.clone()
    }
}

impl Drop for GraphTransport {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct WorkerStartup {
    commands: mpsc::Sender<GetCommand>,
    command_receiver: mpsc::Receiver<GetCommand>,
    shutdown: oneshot::Sender<()>,
    shutdown_receiver: oneshot::Receiver<()>,
    ready: sync_mpsc::SyncSender<Result<(), GraphError>>,
    ready_receiver: sync_mpsc::Receiver<Result<(), GraphError>>,
}

impl WorkerStartup {
    fn new() -> Self {
        let (commands, command_receiver) = mpsc::channel(REQUEST_CHANNEL_CAPACITY);
        let (shutdown, shutdown_receiver) = oneshot::channel();
        let (ready, ready_receiver) = sync_mpsc::sync_channel(1);
        Self {
            commands,
            command_receiver,
            shutdown,
            shutdown_receiver,
            ready,
            ready_receiver,
        }
    }

    fn launch(
        self,
        client_builder: fn() -> Result<Client, GraphError>,
    ) -> Result<GraphTransport, GraphError> {
        let Self {
            commands,
            command_receiver,
            shutdown,
            shutdown_receiver,
            ready,
            ready_receiver,
        } = self;
        let worker = thread::Builder::new()
            .name("einfach-mail-graph-transport".to_owned())
            .spawn(move || {
                worker_thread(command_receiver, shutdown_receiver, ready, client_builder)
            })
            .map_err(|_| GraphError::WorkerStart)?;
        finish_startup(commands, shutdown, ready_receiver, worker)
    }
}

fn finish_startup(
    commands: mpsc::Sender<GetCommand>,
    shutdown: oneshot::Sender<()>,
    ready: sync_mpsc::Receiver<Result<(), GraphError>>,
    worker: JoinHandle<()>,
) -> Result<GraphTransport, GraphError> {
    match ready.recv() {
        Ok(Ok(())) => Ok(GraphTransport {
            handle: GraphTransportHandle { commands },
            shutdown: Some(shutdown),
            worker: Some(worker),
        }),
        Ok(Err(error)) => join_after_startup_error(worker, error),
        Err(_) => join_after_startup_error(worker, GraphError::WorkerClosed),
    }
}

fn join_after_startup_error(
    worker: JoinHandle<()>,
    error: GraphError,
) -> Result<GraphTransport, GraphError> {
    let _ = worker.join();
    Err(error)
}

fn worker_thread(
    commands: mpsc::Receiver<GetCommand>,
    shutdown: oneshot::Receiver<()>,
    ready: sync_mpsc::SyncSender<Result<(), GraphError>>,
    client_builder: fn() -> Result<Client, GraphError>,
) {
    let (runtime, client) = match worker_resources(client_builder) {
        Ok(resources) => resources,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    if ready.send(Ok(())).is_err() {
        return;
    }
    runtime.block_on(run_worker(commands, shutdown, client));
}

fn worker_resources(
    client_builder: fn() -> Result<Client, GraphError>,
) -> Result<(Runtime, Client), GraphError> {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| GraphError::WorkerStart)?;
    let client = client_builder()?;
    Ok((runtime, client))
}

fn build_system_client() -> Result<Client, GraphError> {
    configured_client(http_client::build_client())
}

#[cfg(test)]
fn build_fixture_client() -> Result<Client, GraphError> {
    configured_client(http_client::build_fixture_client())
}

fn configured_client(client: Result<Client, reqwest::Error>) -> Result<Client, GraphError> {
    client.map_err(|_| GraphError::WorkerStart)
}

async fn run_worker(
    mut commands: mpsc::Receiver<GetCommand>,
    mut shutdown: oneshot::Receiver<()>,
    client: Client,
) {
    let mut tasks = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => {
                stop_worker(&mut commands, &mut tasks).await;
                return;
            }
            command = commands.recv(), if tasks.len() < MAX_ACTIVE_REQUESTS => {
                match command {
                    Some(command) => spawn_request(&mut tasks, client.clone(), command),
                    None => {
                        stop_worker(&mut commands, &mut tasks).await;
                        return;
                    }
                }
            }
            completed = tasks.join_next(), if !tasks.is_empty() => {
                if matches!(completed, Some(Err(_))) {
                    stop_worker(&mut commands, &mut tasks).await;
                    return;
                }
            }
        }
    }
}

async fn stop_worker(commands: &mut mpsc::Receiver<GetCommand>, tasks: &mut JoinSet<()>) {
    commands.close();
    tasks.shutdown().await;
}

fn spawn_request(tasks: &mut JoinSet<()>, client: Client, command: GetCommand) {
    tasks.spawn(async move {
        let GetCommand { request, mut reply } = command;
        if let Some(request_result) = request_with_retry(&client, &request, &mut reply).await {
            let _ = reply.send(request_result);
        }
    });
}

async fn request_with_retry(
    client: &Client,
    request: &GraphRequest,
    reply: &mut oneshot::Sender<Result<GraphResponse, GraphError>>,
) -> Option<Result<GraphResponse, GraphError>> {
    let mut retry_index = 0;
    loop {
        let response = match send_once(client, request, reply).await? {
            Ok(response) => response,
            Err(error) => return Some(Err(error)),
        };
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            match retry_decision(&response, retry_index) {
                RetryDecision::Retry(delay) => {
                    drop(response);
                    wait_before_retry(reply, delay).await?;
                    retry_index += 1;
                }
                RetryDecision::Stop(error) => return Some(Err(error)),
            }
        } else {
            return response_result(request.operation, response, reply).await;
        }
    }
}

enum RetryDecision {
    Retry(Duration),
    Stop(GraphError),
}

fn retry_decision(response: &Response, attempt: u32) -> RetryDecision {
    let retry_after = retry_after_delay(
        response.headers().get(RETRY_AFTER),
        SystemTime::now(),
        attempt,
    );
    retry_decision_for(retry_after, attempt)
}

fn retry_decision_for(retry_after: RetryAfter, attempt: u32) -> RetryDecision {
    match retry_after {
        RetryAfter::Delay(delay)
            if delay <= MAX_AUTOMATIC_RETRY_DELAY && attempt < MAX_THROTTLE_RETRIES =>
        {
            RetryDecision::Retry(delay)
        }
        RetryAfter::Delay(delay) => RetryDecision::Stop(GraphError::RateLimited {
            retry_after: Some(delay),
        }),
        RetryAfter::Unrepresentable => {
            RetryDecision::Stop(GraphError::RateLimited { retry_after: None })
        }
    }
}

async fn wait_before_retry(
    reply: &mut oneshot::Sender<Result<GraphResponse, GraphError>>,
    delay: Duration,
) -> Option<()> {
    while_interested(reply, tokio::time::sleep(delay)).await
}

async fn response_result(
    operation: GraphOperation,
    response: Response,
    reply: &mut oneshot::Sender<Result<GraphResponse, GraphError>>,
) -> Option<Result<GraphResponse, GraphError>> {
    if matches!(operation, GraphOperation::MessageDelta) && response.status() == StatusCode::GONE {
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        return Some(Ok(GraphResponse::MessageDeltaReset(location)));
    }
    match response.status() {
        StatusCode::UNAUTHORIZED => Some(Err(GraphError::AccessTokenRejected)),
        StatusCode::FORBIDDEN => Some(Err(GraphError::AccessDenied)),
        status if status.is_success() => read_limited_body(response, reply).await,
        _ => Some(Err(GraphError::Transport)),
    }
}

async fn read_limited_body(
    mut response: Response,
    reply: &mut oneshot::Sender<Result<GraphResponse, GraphError>>,
) -> Option<Result<GraphResponse, GraphError>> {
    if response_body_is_too_large(&response) {
        return Some(Err(GraphError::ResponseTooLarge));
    }
    let mut body = Vec::new();
    loop {
        match while_interested(reply, response.chunk()).await? {
            Ok(Some(chunk)) => match append_response_chunk(&mut body, &chunk) {
                Ok(()) => {}
                Err(error) => return Some(Err(error)),
            },
            Ok(None) => return Some(Ok(GraphResponse::Body(body))),
            Err(_) => return Some(Err(GraphError::Transport)),
        }
    }
}

fn response_body_is_too_large(response: &Response) -> bool {
    response.content_length().is_some_and(|length| {
        usize::try_from(length).map_or(true, |length| length > MAX_RESPONSE_BODY_BYTES)
    })
}

fn append_response_chunk(body: &mut Vec<u8>, chunk: &[u8]) -> Result<(), GraphError> {
    if chunk.len() > MAX_RESPONSE_BODY_BYTES - body.len() {
        return Err(GraphError::ResponseTooLarge);
    }
    body.extend_from_slice(chunk);
    Ok(())
}

async fn send_once(
    client: &Client,
    request: &GraphRequest,
    reply: &mut oneshot::Sender<Result<GraphResponse, GraphError>>,
) -> Option<Result<Response, GraphError>> {
    let authorization = match authorization_header(&request.token) {
        Ok(header) => header,
        Err(error) => return Some(Err(error)),
    };
    let mut outbound = client
        .get(request.url.as_str())
        .header(AUTHORIZATION, authorization);
    if request.operation.uses_immutable_ids() {
        outbound = outbound.header("Prefer", "IdType=\"ImmutableId\"");
    }
    while_interested(reply, outbound.send())
        .await?
        .map_err(|_| GraphError::Transport)
        .into()
}

fn authorization_header(token: &AccessToken) -> Result<HeaderValue, GraphError> {
    let mut header = token
        .expose_header_value()
        .map_err(|_| GraphError::InvalidToken)?;
    header.set_sensitive(true);
    Ok(header)
}

async fn while_interested<T, F>(
    reply: &mut oneshot::Sender<Result<GraphResponse, GraphError>>,
    work: F,
) -> Option<T>
where
    F: Future<Output = T>,
{
    tokio::select! {
        biased;
        () = reply.closed() => None,
        work_output = work => Some(work_output),
    }
}

fn retry_after_delay(
    retry_after_header: Option<&HeaderValue>,
    now: SystemTime,
    attempt: u32,
) -> RetryAfter {
    let Some(retry_after_text) = retry_after_header.and_then(|header| header.to_str().ok()) else {
        return RetryAfter::Delay(fallback_delay(attempt));
    };
    parse_retry_after(retry_after_text, now, attempt)
}

fn parse_retry_after(retry_after_text: &str, now: SystemTime, attempt: u32) -> RetryAfter {
    if !retry_after_text.is_empty() && retry_after_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return match retry_after_text.parse::<u64>() {
            Ok(seconds) => RetryAfter::Delay(Duration::from_secs(seconds)),
            Err(_) => RetryAfter::Unrepresentable,
        };
    }
    match httpdate::parse_http_date(retry_after_text) {
        Ok(retry_at) => RetryAfter::Delay(retry_at.duration_since(now).unwrap_or(Duration::ZERO)),
        Err(_) => RetryAfter::Delay(fallback_delay(attempt)),
    }
}

fn fallback_delay(attempt: u32) -> Duration {
    Duration::from_secs(2_u64.saturating_pow(attempt).min(60))
}

#[cfg(test)]
mod tests {
    use std::{
        process::Command,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::{Duration, Instant, SystemTime},
    };

    use httpmock::{HttpMockRequest, HttpMockResponse, Method::GET, Mock, MockServer};
    use huskarl::{
        core::secrets::SecretString,
        token::{AccessToken, BearerAccessToken},
    };
    use tokio::time::timeout;
    use url::Url;

    use super::{
        GraphOperation, GraphResponse, GraphTransport, RetryAfter, RetryDecision,
        append_response_chunk, authorization_header, fallback_delay, parse_retry_after,
        retry_decision_for,
    };
    use crate::{error::GraphError, graph_url::GraphUrl};

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
                GraphOperation::Profile,
                graph_url(&server.url("/oversized")),
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
            server.base_url()
        );
        let response = server
            .mock_async(move |when, then| {
                when.method(GET).path("/delta");
                then.status(410).header("Location", location);
            })
            .await;
        let transport = GraphTransport::start_for_fixtures().expect("fixture worker starts");
        let handle = transport.handle();
        let url = graph_url(&server.url("/delta"));

        let reset = handle
            .get_message_delta(url.clone(), synthetic_access_token())
            .await
            .expect("message delta reset is returned to the Graph adapter");
        let other_operation = handle
            .get(GraphOperation::Profile, url, synthetic_access_token())
            .await;

        assert_eq!(
            reset,
            GraphResponse::MessageDeltaReset(Some(format!(
                "{}/v1.0/me/mailFolders/folder/messages/delta?$deltatoken=",
                server.base_url()
            )))
        );
        assert_eq!(other_operation, Err(GraphError::Transport));
        response.assert_calls_async(2).await;
    }

    #[test]
    fn parses_retry_after_seconds_and_dates_without_early_retries() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_798_000_000);

        assert_eq!(
            parse_retry_after("17", now, 0),
            RetryAfter::Delay(Duration::from_secs(17))
        );
        assert_eq!(
            parse_retry_after("99999999999999999999", now, 0),
            RetryAfter::Unrepresentable
        );
        assert_eq!(fallback_delay(0), Duration::from_secs(1));
        assert_eq!(fallback_delay(10), Duration::from_secs(60));
        let date = httpdate::fmt_http_date(now + Duration::from_secs(31));
        assert_eq!(
            parse_retry_after(&date, now, 0),
            RetryAfter::Delay(Duration::from_secs(31))
        );
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
        let slow_url = graph_url(&server.url("/slow"));
        let requests = (0..super::MAX_ACTIVE_REQUESTS)
            .map(|_| {
                let handle = handle.clone();
                let slow_url = slow_url.clone();
                let token = synthetic_access_token();
                tokio::spawn(
                    async move { handle.get(GraphOperation::Profile, slow_url, token).await },
                )
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
                GraphOperation::Profile,
                graph_url(&server.url("/probe")),
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
        let slow_url = graph_url(&server.url("/slow"));
        let request = tokio::spawn(async move {
            handle
                .get(GraphOperation::Profile, slow_url, synthetic_access_token())
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
        let proxy_url = proxy.base_url();
        let child_output = Command::new(
            std::env::current_exe().expect("current test executable is available"),
        )
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
                GraphOperation::Folders,
                graph_url(&server.url("/throttled")),
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
        let url = format!("{}?opaque=synthetic-url-secret", server.url("/failure"));

        let error = transport
            .handle()
            .get(
                GraphOperation::Profile,
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
                GraphOperation::Profile,
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
    fn provider_errors_do_not_contain_request_secrets() {
        let error = GraphError::Transport;
        let rendered = format!("{error:?}");

        assert!(!rendered.contains("synthetic-access-token"));
        assert!(!rendered.contains("https://graph.microsoft.com"));
    }

    #[test]
    fn rate_limit_stops_after_the_retry_budget_or_long_delay() {
        let decision = retry_decision_for(RetryAfter::Delay(Duration::from_secs(120)), 0);

        assert!(
            matches!(decision, RetryDecision::Stop(GraphError::RateLimited { retry_after: Some(delay) }) if delay == Duration::from_secs(120))
        );
    }
}
