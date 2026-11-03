use std::{
    future::Future,
    sync::{Arc, mpsc as sync_mpsc},
    thread::{self, JoinHandle},
    time::{Duration, SystemTime},
};

use huskarl::token::AccessToken;
use reqwest::{
    Client, Response, StatusCode,
    header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue, LOCATION, RETRY_AFTER},
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
    MessageUpdate,
    MessageMove,
    MailSend,
    CalendarViewDelta,
    CalendarEventCreate,
}

impl GraphOperation {
    fn requires_json_content_type(self) -> bool {
        matches!(
            self,
            Self::MessageDelta | Self::MailSend | Self::CalendarViewDelta
        )
    }

    fn uses_immutable_ids(self) -> bool {
        matches!(
            self,
            Self::Messages
                | Self::MessageDelta
                | Self::MessageBody
                | Self::MessageUpdate
                | Self::MessageMove
                | Self::CalendarViewDelta
                | Self::CalendarEventCreate
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
    method: GraphRequestMethod,
    url: GraphUrl,
    token: Arc<AccessToken>,
}

enum GraphRequestMethod {
    Get,
    PatchJson(Arc<[u8]>),
    PostJson(Arc<[u8]>),
}

struct GraphCommand {
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
    commands: mpsc::Sender<GraphCommand>,
}

impl GraphTransportHandle {
    pub(super) async fn get(
        &self,
        operation: GraphOperation,
        url: GraphUrl,
        token: Arc<AccessToken>,
    ) -> Result<Vec<u8>, GraphError> {
        self.request(operation, GraphRequestMethod::Get, url, token)
            .await?
            .into_body()
    }

    pub(super) async fn post_mail(
        &self,
        url: GraphUrl,
        token: Arc<AccessToken>,
        body: Vec<u8>,
    ) -> Result<(), GraphError> {
        match self
            .request(
                GraphOperation::MailSend,
                GraphRequestMethod::PostJson(body.into()),
                url,
                token,
            )
            .await?
        {
            GraphResponse::Body(body) if body.is_empty() => Ok(()),
            _ => Err(GraphError::InvalidResponse),
        }
    }

    pub(super) async fn get_message_delta(
        &self,
        url: GraphUrl,
        token: Arc<AccessToken>,
    ) -> Result<GraphResponse, GraphError> {
        self.request(
            GraphOperation::MessageDelta,
            GraphRequestMethod::Get,
            url,
            token,
        )
        .await
    }

    pub(super) async fn patch_json(
        &self,
        operation: GraphOperation,
        url: GraphUrl,
        token: Arc<AccessToken>,
        body: Arc<[u8]>,
    ) -> Result<Vec<u8>, GraphError> {
        self.request(operation, GraphRequestMethod::PatchJson(body), url, token)
            .await?
            .into_body()
    }

    async fn request(
        &self,
        operation: GraphOperation,
        method: GraphRequestMethod,
        url: GraphUrl,
        token: Arc<AccessToken>,
    ) -> Result<GraphResponse, GraphError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(GraphCommand {
                request: GraphRequest {
                    operation,
                    method,
                    url,
                    token,
                },
                reply,
            })
            .await
            .map_err(|_| GraphError::WorkerClosed)?;
        response.await.map_err(|_| match operation {
            GraphOperation::MailSend => GraphError::SendOutcomeUnknown,
            GraphOperation::MessageUpdate => GraphError::MessageUpdateOutcomeUnknown,
            GraphOperation::MessageMove => GraphError::MessageMoveOutcomeUnknown,
            GraphOperation::CalendarEventCreate => GraphError::CalendarEventCreationOutcomeUnknown,
            _ => GraphError::WorkerClosed,
        })?
    }

    pub(super) async fn post_json(
        &self,
        operation: GraphOperation,
        url: GraphUrl,
        token: Arc<AccessToken>,
        body: impl Into<Arc<[u8]>>,
    ) -> Result<Vec<u8>, GraphError> {
        self.request(
            operation,
            GraphRequestMethod::PostJson(body.into()),
            url,
            token,
        )
        .await?
        .into_body()
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
    commands: mpsc::Sender<GraphCommand>,
    command_receiver: mpsc::Receiver<GraphCommand>,
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
    commands: mpsc::Sender<GraphCommand>,
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
    commands: mpsc::Receiver<GraphCommand>,
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
    mut commands: mpsc::Receiver<GraphCommand>,
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

async fn stop_worker(commands: &mut mpsc::Receiver<GraphCommand>, tasks: &mut JoinSet<()>) {
    commands.close();
    tasks.shutdown().await;
}

fn spawn_request(tasks: &mut JoinSet<()>, client: Client, command: GraphCommand) {
    tasks.spawn(async move {
        let GraphCommand { request, mut reply } = command;
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
            Err(GraphError::Transport)
                if matches!(request.operation, GraphOperation::MessageUpdate) =>
            {
                return Some(Err(GraphError::MessageUpdateOutcomeUnknown));
            }
            Err(GraphError::Transport) if matches!(request.operation, GraphOperation::MailSend) => {
                return Some(Err(GraphError::SendOutcomeUnknown));
            }
            Err(GraphError::Transport)
                if matches!(request.operation, GraphOperation::MessageMove) =>
            {
                return Some(Err(GraphError::MessageMoveOutcomeUnknown));
            }
            Err(GraphError::Transport)
                if matches!(request.operation, GraphOperation::CalendarEventCreate) =>
            {
                return Some(Err(GraphError::CalendarEventCreationOutcomeUnknown));
            }
            Err(error) => return Some(Err(error)),
        };
        if matches!(request.operation, GraphOperation::MessageMove) {
            return message_move_response_result(response, reply).await;
        }
        if matches!(request.operation, GraphOperation::MessageUpdate) {
            return message_update_response_result(response, reply).await;
        }
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            if matches!(request.operation, GraphOperation::MailSend) {
                return response_result(request.operation, response, reply).await;
            }
            match &request.method {
                GraphRequestMethod::Get => match retry_decision(&response, retry_index) {
                    RetryDecision::Retry(delay) => {
                        drop(response);
                        wait_before_retry(reply, delay).await?;
                        retry_index += 1;
                    }
                    RetryDecision::Stop(error) => return Some(Err(error)),
                },
                GraphRequestMethod::PostJson(_)
                    if matches!(request.operation, GraphOperation::CalendarEventCreate) =>
                {
                    return Some(Err(GraphError::CalendarEventCreationOutcomeUnknown));
                }
                GraphRequestMethod::PatchJson(_) | GraphRequestMethod::PostJson(_) => {
                    return Some(Err(rate_limit_error(&response)));
                }
            }
        } else if !has_expected_success_status(request.operation, response.status()) {
            return Some(Err(GraphError::InvalidResponse));
        } else {
            return response_result(request.operation, response, reply).await;
        }
    }
}

fn has_expected_success_status(operation: GraphOperation, status: StatusCode) -> bool {
    if !status.is_success() {
        return true;
    }
    match operation {
        GraphOperation::CalendarEventCreate => status == StatusCode::CREATED,
        _ => true,
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

fn rate_limit_error(response: &Response) -> GraphError {
    match retry_after_delay(response.headers().get(RETRY_AFTER), SystemTime::now(), 0) {
        RetryAfter::Delay(delay) => GraphError::RateLimited {
            retry_after: Some(delay),
        },
        RetryAfter::Unrepresentable => GraphError::RateLimited { retry_after: None },
    }
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
    if matches!(operation, GraphOperation::MailSend) {
        return Some(match response.status() {
            StatusCode::ACCEPTED => Ok(GraphResponse::Body(Vec::new())),
            StatusCode::UNAUTHORIZED => Err(GraphError::AccessTokenRejected),
            StatusCode::FORBIDDEN => Err(GraphError::AccessDenied),
            StatusCode::TOO_MANY_REQUESTS => Err(GraphError::RateLimited {
                retry_after: retry_after_hint(
                    response.headers().get(RETRY_AFTER),
                    SystemTime::now(),
                ),
            }),
            StatusCode::REQUEST_TIMEOUT => Err(GraphError::SendOutcomeUnknown),
            status if status.is_client_error() => Err(GraphError::SendRejected),
            _ => Err(GraphError::SendOutcomeUnknown),
        });
    }
    if matches!(operation, GraphOperation::CalendarEventCreate) {
        return calendar_event_create_response_result(response, reply).await;
    }
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

async fn calendar_event_create_response_result(
    response: Response,
    reply: &mut oneshot::Sender<Result<GraphResponse, GraphError>>,
) -> Option<Result<GraphResponse, GraphError>> {
    match response.status() {
        StatusCode::UNAUTHORIZED => Some(Err(GraphError::AccessTokenRejected)),
        StatusCode::FORBIDDEN => Some(Err(GraphError::AccessDenied)),
        StatusCode::REQUEST_TIMEOUT | StatusCode::CONFLICT => {
            Some(Err(GraphError::CalendarEventCreationOutcomeUnknown))
        }
        status if status.is_server_error() => {
            Some(Err(GraphError::CalendarEventCreationOutcomeUnknown))
        }
        StatusCode::CREATED => match read_limited_body(response, reply).await? {
            Ok(body) => Some(Ok(body)),
            Err(_) => Some(Err(GraphError::CalendarEventCreationOutcomeUnknown)),
        },
        status if status.is_client_error() => Some(Err(GraphError::CalendarEventCreationRejected)),
        _ => Some(Err(GraphError::CalendarEventCreationOutcomeUnknown)),
    }
}

/// Returns a single move response, preserving uncertainty when Graph may have applied it.
async fn message_move_response_result(
    response: Response,
    reply: &mut oneshot::Sender<Result<GraphResponse, GraphError>>,
) -> Option<Result<GraphResponse, GraphError>> {
    let status = response.status();
    if status == StatusCode::CREATED {
        return match read_limited_body(response, reply).await? {
            Ok(body) => Some(Ok(body)),
            Err(_) => Some(Err(GraphError::MessageMoveOutcomeUnknown)),
        };
    }
    let error = match status {
        StatusCode::UNAUTHORIZED => Err(GraphError::AccessTokenRejected),
        StatusCode::FORBIDDEN => Err(GraphError::AccessDenied),
        StatusCode::TOO_MANY_REQUESTS => Err(rate_limit_error(&response)),
        StatusCode::REQUEST_TIMEOUT => Err(GraphError::MessageMoveOutcomeUnknown),
        status if status.is_server_error() || status.is_success() || status.is_redirection() => {
            Err(GraphError::MessageMoveOutcomeUnknown)
        }
        _ => Err(GraphError::Transport),
    };
    Some(error)
}

async fn message_update_response_result(
    response: Response,
    reply: &mut oneshot::Sender<Result<GraphResponse, GraphError>>,
) -> Option<Result<GraphResponse, GraphError>> {
    match response.status() {
        StatusCode::UNAUTHORIZED => Some(Err(GraphError::AccessTokenRejected)),
        StatusCode::FORBIDDEN => Some(Err(GraphError::AccessDenied)),
        StatusCode::TOO_MANY_REQUESTS => Some(Err(rate_limit_error(&response))),
        StatusCode::OK => Some(match read_limited_body(response, reply).await? {
            Ok(body) => Ok(body),
            Err(_) => Err(GraphError::MessageUpdateOutcomeUnknown),
        }),
        status if message_update_outcome_may_be_unknown(status) => {
            Some(Err(GraphError::MessageUpdateOutcomeUnknown))
        }
        _ => Some(Err(GraphError::Transport)),
    }
}

fn message_update_outcome_may_be_unknown(status: StatusCode) -> bool {
    status.is_success()
        || status.is_server_error()
        || status.is_redirection()
        || status == StatusCode::REQUEST_TIMEOUT
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
    let mut outbound = match (&request.method, request.operation) {
        (GraphRequestMethod::Get, operation)
            if !matches!(
                operation,
                GraphOperation::MailSend
                    | GraphOperation::MessageUpdate
                    | GraphOperation::MessageMove
                    | GraphOperation::CalendarEventCreate
            ) =>
        {
            client.get(request.url.as_str())
        }
        (GraphRequestMethod::PostJson(body), operation)
            if matches!(
                operation,
                GraphOperation::MailSend
                    | GraphOperation::MessageMove
                    | GraphOperation::CalendarEventCreate
            ) =>
        {
            client
                .post(request.url.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Arc::clone(body).to_vec())
        }
        (GraphRequestMethod::PatchJson(body), GraphOperation::MessageUpdate) => client
            .patch(request.url.as_str())
            .header(CONTENT_TYPE, "application/json")
            .body(Arc::clone(body).to_vec()),
        _ => return Some(Err(GraphError::InvalidResponse)),
    }
    .header(AUTHORIZATION, authorization);
    if matches!(&request.method, GraphRequestMethod::Get)
        && request.operation.requires_json_content_type()
    {
        outbound = outbound.header(CONTENT_TYPE, "application/json");
    }
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
    retry_after_header
        .and_then(|header| header.to_str().ok())
        .and_then(|retry_after_text| parse_retry_after(retry_after_text, now))
        .unwrap_or_else(|| RetryAfter::Delay(fallback_delay(attempt)))
}

fn retry_after_hint(retry_after_header: Option<&HeaderValue>, now: SystemTime) -> Option<Duration> {
    let retry_after_text = retry_after_header?.to_str().ok()?;
    match parse_retry_after(retry_after_text, now)? {
        RetryAfter::Delay(delay) => Some(delay),
        RetryAfter::Unrepresentable => None,
    }
}

fn parse_retry_after(retry_after_text: &str, now: SystemTime) -> Option<RetryAfter> {
    if !retry_after_text.is_empty() && retry_after_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Some(match retry_after_text.parse::<u64>() {
            Ok(seconds) => RetryAfter::Delay(Duration::from_secs(seconds)),
            Err(_) => RetryAfter::Unrepresentable,
        });
    }
    match httpdate::parse_http_date(retry_after_text) {
        Ok(retry_at) => Some(RetryAfter::Delay(
            retry_at.duration_since(now).unwrap_or(Duration::ZERO),
        )),
        Err(_) => None,
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

    use httpmock::{
        HttpMockRequest, HttpMockResponse,
        Method::{GET, PATCH, POST},
        Mock, MockServer,
    };
    use huskarl::{
        core::secrets::SecretString,
        token::{AccessToken, BearerAccessToken},
    };
    use tokio::{io::AsyncReadExt, net::TcpListener, time::timeout};
    use url::Url;

    use super::{
        GraphOperation, GraphResponse, GraphTransport, RetryAfter, RetryDecision,
        append_response_chunk, authorization_header, fallback_delay, parse_retry_after,
        retry_after_hint, retry_decision_for,
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
        let url = graph_url(&server.url("/v1.0/me/messages/synthetic-message"));
        let request = tokio::spawn(async move {
            handle
                .patch_json(
                    GraphOperation::MessageUpdate,
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
                GraphOperation::MessageUpdate,
                graph_url(&server.url("/v1.0/me/messages/synthetic-message")),
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
                    GraphOperation::MessageUpdate,
                    graph_url(&server.url("/v1.0/me/messages/synthetic-message")),
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
                GraphOperation::MessageMove,
                graph_url(&server.url("/v1.0/me/messages/synthetic-message/move")),
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
                    GraphOperation::MessageMove,
                    graph_url(&server.url("/v1.0/me/messages/synthetic-message/move")),
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
                GraphOperation::MessageUpdate,
                graph_url(&server.url("/v1.0/me/messages/synthetic-message")),
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
                    graph_url(&server.url("/send")),
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
                    graph_url(&server.url("/send")),
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
                graph_url(&server.url("/send")),
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
                GraphOperation::CalendarEventCreate,
                graph_url(&server.url("/v1.0/me/calendar/events")),
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
                GraphOperation::CalendarEventCreate,
                graph_url(&server.url("/v1.0/me/calendar/events")),
                synthetic_access_token(),
                b"{}".to_vec(),
            )
            .await;

        assert_eq!(result, Err(GraphError::CalendarEventCreationOutcomeUnknown));
        post.assert_async().await;
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
                    GraphOperation::CalendarEventCreate,
                    graph_url(&server.url("/v1.0/me/calendar/events")),
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
                GraphOperation::CalendarEventCreate,
                graph_url(&server.url("/v1.0/me/calendar/events")),
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
