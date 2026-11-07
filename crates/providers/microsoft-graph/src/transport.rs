use std::{
    future::Future,
    sync::{Arc, mpsc as sync_mpsc},
    thread::{self, JoinHandle},
    time::{Duration, SystemTime},
};

use huskarl::token::AccessToken;
use reqwest::{
    Client, Response, StatusCode,
    header::{
        AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue, IF_MATCH, LOCATION, RETRY_AFTER,
    },
};
use tokio::{
    runtime::{Builder, Runtime},
    sync::{mpsc, oneshot},
    task::JoinSet,
};

use crate::{
    error::GraphError,
    graph_url::{GraphUrl, MAX_GRAPH_CALENDAR_DELTA_URL_BYTES},
    http_client,
};

#[cfg(test)]
mod tests;

const REQUEST_CHANNEL_CAPACITY: usize = 16;
const MAX_ACTIVE_REQUESTS: usize = 16;
const MAX_RESPONSE_BODY_BYTES: usize = 8 * 1024 * 1024;
const MAX_THROTTLE_RETRIES: u32 = 2;
const MAX_AUTOMATIC_RETRY_DELAY: Duration = Duration::from_secs(60);

#[derive(Clone, Copy)]
pub(super) enum GraphReadOperation {
    Profile,
    Folders,
    Messages,
    MessageBody,
    CalendarEventRead,
}

#[derive(Clone, Copy)]
pub(super) enum GraphCalendarDeltaOperation {
    Initial,
    Continuation,
}

#[derive(Clone, Copy)]
pub(super) enum GraphPostJsonOperation {
    MessageMove,
    CalendarEventCreate,
}

#[derive(Clone, Copy)]
pub(super) enum GraphPatchOperation {
    MessageUpdate,
    CalendarEventUpdate,
}

enum GraphRequestOperation {
    Get(GraphReadOperation),
    MessageDelta,
    CalendarViewDelta(GraphCalendarDeltaOperation),
    SendMail(Arc<[u8]>),
    PostJson {
        operation: GraphPostJsonOperation,
        body: Arc<[u8]>,
    },
    PatchJson {
        operation: GraphPatchOperation,
        body: Arc<[u8]>,
    },
    DeleteCalendarEvent(GraphEventEtag),
}

impl GraphRequestOperation {
    fn requires_json_content_type(&self) -> bool {
        matches!(self, Self::MessageDelta | Self::CalendarViewDelta(_))
    }

    fn uses_immutable_ids(&self) -> bool {
        match self {
            Self::Get(GraphReadOperation::Profile | GraphReadOperation::Folders)
            | Self::SendMail(_) => false,
            Self::Get(
                GraphReadOperation::Messages
                | GraphReadOperation::MessageBody
                | GraphReadOperation::CalendarEventRead,
            )
            | Self::MessageDelta
            | Self::CalendarViewDelta(_)
            | Self::PostJson {
                operation:
                    GraphPostJsonOperation::MessageMove | GraphPostJsonOperation::CalendarEventCreate,
                ..
            }
            | Self::PatchJson {
                operation:
                    GraphPatchOperation::MessageUpdate | GraphPatchOperation::CalendarEventUpdate,
                ..
            }
            | Self::DeleteCalendarEvent(_) => true,
        }
    }

    fn uncertain_write_error(&self) -> Option<GraphError> {
        match self {
            Self::SendMail(_) => Some(GraphError::SendOutcomeUnknown),
            Self::PostJson {
                operation: GraphPostJsonOperation::MessageMove,
                ..
            } => Some(GraphError::MessageMoveOutcomeUnknown),
            Self::PostJson {
                operation: GraphPostJsonOperation::CalendarEventCreate,
                ..
            } => Some(GraphError::CalendarEventCreationOutcomeUnknown),
            Self::PatchJson {
                operation: GraphPatchOperation::MessageUpdate,
                ..
            } => Some(GraphError::MessageUpdateOutcomeUnknown),
            Self::PatchJson {
                operation: GraphPatchOperation::CalendarEventUpdate,
                ..
            } => Some(GraphError::CalendarEventUpdateOutcomeUnknown),
            Self::DeleteCalendarEvent(_) => Some(GraphError::CalendarEventDeletionOutcomeUnknown),
            Self::Get(_) | Self::MessageDelta | Self::CalendarViewDelta(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RetryAfter {
    Delay(Duration),
    Unrepresentable,
}

struct GraphRequest {
    operation: GraphRequestOperation,
    url: GraphUrl,
    token: Arc<AccessToken>,
}

struct GraphEventEtag(HeaderValue);

impl TryFrom<&str> for GraphEventEtag {
    type Error = GraphError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let etag = value
            .parse::<headers::ETag>()
            .map_err(|_| GraphError::InvalidResponse)?;
        let if_match = headers::IfMatch::from(etag);
        let mut values = Vec::new();
        headers::Header::encode(&if_match, &mut values);
        values.pop().map(Self).ok_or(GraphError::InvalidResponse)
    }
}

struct GraphCommand {
    request: GraphRequest,
    reply: oneshot::Sender<Result<GraphResponse, GraphError>>,
}

#[derive(Eq, PartialEq)]
pub(super) enum GraphResponse {
    Body(Vec<u8>),
    MessageDeltaReset(Option<String>),
    CalendarViewDeltaReset {
        location: Option<String>,
        response_bytes: usize,
    },
}

impl std::fmt::Debug for GraphResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Body(_) => formatter.write_str("Body(<redacted>)"),
            Self::MessageDeltaReset(_) => formatter.write_str("MessageDeltaReset(<redacted>)"),
            Self::CalendarViewDeltaReset { .. } => {
                formatter.write_str("CalendarViewDeltaReset(<redacted>)")
            }
        }
    }
}

impl GraphResponse {
    fn into_body(self) -> Result<Vec<u8>, GraphError> {
        match self {
            Self::Body(body) => Ok(body),
            Self::MessageDeltaReset(_) | Self::CalendarViewDeltaReset { .. } => {
                Err(GraphError::InvalidResponse)
            }
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
        operation: GraphReadOperation,
        url: GraphUrl,
        token: Arc<AccessToken>,
    ) -> Result<Vec<u8>, GraphError> {
        self.request(GraphRequestOperation::Get(operation), url, token)
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
            .request(GraphRequestOperation::SendMail(body.into()), url, token)
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
        self.request(GraphRequestOperation::MessageDelta, url, token)
            .await
    }

    pub(super) async fn patch_json(
        &self,
        operation: GraphPatchOperation,
        url: GraphUrl,
        token: Arc<AccessToken>,
        body: Arc<[u8]>,
    ) -> Result<Vec<u8>, GraphError> {
        self.request(
            GraphRequestOperation::PatchJson { operation, body },
            url,
            token,
        )
        .await?
        .into_body()
    }

    pub(super) async fn delete_calendar_event(
        &self,
        url: GraphUrl,
        token: Arc<AccessToken>,
        etag: &str,
    ) -> Result<(), GraphError> {
        let etag = GraphEventEtag::try_from(etag)?;
        match self
            .request(GraphRequestOperation::DeleteCalendarEvent(etag), url, token)
            .await?
        {
            GraphResponse::Body(body) if body.is_empty() => Ok(()),
            _ => Err(GraphError::CalendarEventDeletionOutcomeUnknown),
        }
    }

    pub(super) async fn get_calendar_delta(
        &self,
        operation: GraphCalendarDeltaOperation,
        url: GraphUrl,
        token: Arc<AccessToken>,
    ) -> Result<GraphResponse, GraphError> {
        self.request(
            GraphRequestOperation::CalendarViewDelta(operation),
            url,
            token,
        )
        .await
    }

    async fn request(
        &self,
        operation: GraphRequestOperation,
        url: GraphUrl,
        token: Arc<AccessToken>,
    ) -> Result<GraphResponse, GraphError> {
        let (reply, response) = oneshot::channel();
        let worker_closed_error = operation
            .uncertain_write_error()
            .unwrap_or(GraphError::WorkerClosed);
        self.commands
            .send(GraphCommand {
                request: GraphRequest {
                    operation,
                    url,
                    token,
                },
                reply,
            })
            .await
            .map_err(|_| GraphError::WorkerClosed)?;
        response.await.map_err(|_| worker_closed_error)?
    }

    pub(super) async fn post_json(
        &self,
        operation: GraphPostJsonOperation,
        url: GraphUrl,
        token: Arc<AccessToken>,
        body: impl Into<Arc<[u8]>>,
    ) -> Result<Vec<u8>, GraphError> {
        self.request(
            GraphRequestOperation::PostJson {
                operation,
                body: body.into(),
            },
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
            Err(GraphError::Transport) => {
                return Some(Err(request
                    .operation
                    .uncertain_write_error()
                    .unwrap_or(GraphError::Transport)));
            }
            Err(error) => return Some(Err(error)),
        };
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            match &request.operation {
                GraphRequestOperation::Get(_)
                | GraphRequestOperation::MessageDelta
                | GraphRequestOperation::CalendarViewDelta(_) => {
                    match retry_decision(&response, retry_index) {
                        RetryDecision::Retry(delay) => {
                            drop(response);
                            wait_before_retry(reply, delay).await?;
                            retry_index += 1;
                        }
                        RetryDecision::Stop(error) => return Some(Err(error)),
                    }
                }
                GraphRequestOperation::SendMail(_)
                | GraphRequestOperation::PostJson {
                    operation: GraphPostJsonOperation::MessageMove,
                    ..
                }
                | GraphRequestOperation::PatchJson {
                    operation: GraphPatchOperation::MessageUpdate,
                    ..
                } => {
                    return response_result(&request.operation, response, reply).await;
                }
                GraphRequestOperation::PostJson {
                    operation: GraphPostJsonOperation::CalendarEventCreate,
                    ..
                } => {
                    return Some(Err(GraphError::CalendarEventCreationOutcomeUnknown));
                }
                GraphRequestOperation::PatchJson {
                    operation: GraphPatchOperation::CalendarEventUpdate,
                    ..
                }
                | GraphRequestOperation::DeleteCalendarEvent(_) => {
                    return Some(Err(rate_limit_error(&response)));
                }
            }
        } else {
            return response_result(&request.operation, response, reply).await;
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
    operation: &GraphRequestOperation,
    response: Response,
    reply: &mut oneshot::Sender<Result<GraphResponse, GraphError>>,
) -> Option<Result<GraphResponse, GraphError>> {
    match operation {
        GraphRequestOperation::SendMail(_) => Some(match response.status() {
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
        }),
        GraphRequestOperation::PostJson {
            operation: GraphPostJsonOperation::MessageMove,
            ..
        } => message_move_response_result(response, reply).await,
        GraphRequestOperation::PostJson {
            operation: GraphPostJsonOperation::CalendarEventCreate,
            ..
        } => calendar_event_create_response_result(response, reply).await,
        GraphRequestOperation::PatchJson {
            operation: GraphPatchOperation::MessageUpdate,
            ..
        } => message_update_response_result(response, reply).await,
        GraphRequestOperation::PatchJson {
            operation: GraphPatchOperation::CalendarEventUpdate,
            ..
        } => calendar_event_update_response_result(response, reply).await,
        GraphRequestOperation::Get(GraphReadOperation::CalendarEventRead)
            if response.status() == StatusCode::NOT_FOUND =>
        {
            Some(Err(GraphError::CalendarEventChanged))
        }
        GraphRequestOperation::Get(
            GraphReadOperation::Profile
            | GraphReadOperation::Folders
            | GraphReadOperation::Messages
            | GraphReadOperation::MessageBody
            | GraphReadOperation::CalendarEventRead,
        )
        | GraphRequestOperation::CalendarViewDelta(GraphCalendarDeltaOperation::Initial) => {
            standard_response_result(response, reply).await
        }
        GraphRequestOperation::MessageDelta if response.status() == StatusCode::GONE => {
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            Some(Ok(GraphResponse::MessageDeltaReset(location)))
        }
        GraphRequestOperation::MessageDelta => standard_response_result(response, reply).await,
        GraphRequestOperation::CalendarViewDelta(GraphCalendarDeltaOperation::Continuation) => {
            calendar_delta_response_result(response, reply).await
        }
        GraphRequestOperation::DeleteCalendarEvent(_) => {
            calendar_event_delete_response_result(response)
        }
    }
}

async fn standard_response_result(
    response: Response,
    reply: &mut oneshot::Sender<Result<GraphResponse, GraphError>>,
) -> Option<Result<GraphResponse, GraphError>> {
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

async fn calendar_event_update_response_result(
    response: Response,
    reply: &mut oneshot::Sender<Result<GraphResponse, GraphError>>,
) -> Option<Result<GraphResponse, GraphError>> {
    match response.status() {
        StatusCode::UNAUTHORIZED => Some(Err(GraphError::AccessTokenRejected)),
        StatusCode::FORBIDDEN => Some(Err(GraphError::AccessDenied)),
        StatusCode::TOO_MANY_REQUESTS => Some(Err(rate_limit_error(&response))),
        StatusCode::NOT_FOUND => Some(Err(GraphError::CalendarEventChanged)),
        StatusCode::REQUEST_TIMEOUT => Some(Err(GraphError::CalendarEventUpdateOutcomeUnknown)),
        status if status.is_server_error() || status.is_redirection() => {
            Some(Err(GraphError::CalendarEventUpdateOutcomeUnknown))
        }
        StatusCode::OK => match read_limited_body(response, reply).await? {
            Ok(body) => Some(Ok(body)),
            Err(_) => Some(Err(GraphError::CalendarEventUpdateOutcomeUnknown)),
        },
        status if status.is_success() => Some(Err(GraphError::CalendarEventUpdateOutcomeUnknown)),
        _ => Some(Err(GraphError::InvalidResponse)),
    }
}

fn calendar_event_delete_response_result(
    response: Response,
) -> Option<Result<GraphResponse, GraphError>> {
    let result = match response.status() {
        StatusCode::NO_CONTENT => Ok(GraphResponse::Body(Vec::new())),
        StatusCode::UNAUTHORIZED => Err(GraphError::AccessTokenRejected),
        StatusCode::FORBIDDEN => Err(GraphError::AccessDenied),
        StatusCode::TOO_MANY_REQUESTS => Err(rate_limit_error(&response)),
        StatusCode::NOT_FOUND | StatusCode::PRECONDITION_FAILED => {
            Err(GraphError::CalendarEventChanged)
        }
        StatusCode::REQUEST_TIMEOUT => Err(GraphError::CalendarEventDeletionOutcomeUnknown),
        status if status.is_server_error() || status.is_redirection() || status.is_success() => {
            Err(GraphError::CalendarEventDeletionOutcomeUnknown)
        }
        status if status.is_client_error() => Err(GraphError::CalendarEventDeletionRejected),
        _ => Err(GraphError::CalendarEventDeletionOutcomeUnknown),
    };
    Some(result)
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

async fn calendar_delta_response_result(
    response: Response,
    reply: &mut oneshot::Sender<Result<GraphResponse, GraphError>>,
) -> Option<Result<GraphResponse, GraphError>> {
    if response.status() == StatusCode::GONE {
        let location = match calendar_reset_location(response.headers()) {
            Ok(Some(location)) => location,
            Ok(None) => return Some(Err(GraphError::Transport)),
            Err(error) => return Some(Err(error)),
        };
        return Some(calendar_delta_reset(Some(location), 0));
    }

    match response.status() {
        StatusCode::UNAUTHORIZED => Some(Err(GraphError::AccessTokenRejected)),
        StatusCode::FORBIDDEN => Some(Err(GraphError::AccessDenied)),
        status if status.is_success() => read_limited_body(response, reply).await,
        status if status.is_client_error() => {
            let location = calendar_reset_location(response.headers());
            match read_limited_body(response, reply).await? {
                Ok(GraphResponse::Body(body)) if is_sync_state_not_found(&body) => {
                    let location = match location {
                        Ok(location) => location,
                        Err(error) => return Some(Err(error)),
                    };
                    Some(calendar_delta_reset(location, body.len()))
                }
                Ok(_) => Some(Err(GraphError::Transport)),
                Err(GraphError::ResponseTooLarge) => Some(Err(GraphError::Transport)),
                Err(error) => Some(Err(error)),
            }
        }
        _ => Some(Err(GraphError::Transport)),
    }
}

fn calendar_reset_location(headers: &HeaderMap) -> Result<Option<String>, GraphError> {
    let mut locations = headers.get_all(LOCATION).iter();
    let Some(location) = locations.next() else {
        return Ok(None);
    };
    if locations.next().is_some() {
        return Err(GraphError::UnsafeLink);
    }
    let location = location.to_str().map_err(|_| GraphError::UnsafeLink)?;
    if location.len() > MAX_GRAPH_CALENDAR_DELTA_URL_BYTES {
        return Err(GraphError::ResponseTooLarge);
    }
    if location.trim() != location {
        return Err(GraphError::UnsafeLink);
    }
    Ok(Some(location.to_owned()))
}

fn calendar_delta_reset(
    location: Option<String>,
    body_bytes: usize,
) -> Result<GraphResponse, GraphError> {
    let response_bytes = body_bytes
        .checked_add(location.as_ref().map_or(0, String::len))
        .ok_or(GraphError::ResponseTooLarge)?;
    Ok(GraphResponse::CalendarViewDeltaReset {
        location,
        response_bytes,
    })
}

fn is_sync_state_not_found(body: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(body).is_ok_and(|response| {
        response
            .pointer("/error/code")
            .and_then(serde_json::Value::as_str)
            == Some("syncStateNotFound")
    })
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
    let mut outbound = match &request.operation {
        GraphRequestOperation::Get(_)
        | GraphRequestOperation::MessageDelta
        | GraphRequestOperation::CalendarViewDelta(_) => client.get(request.url.as_str()),
        GraphRequestOperation::SendMail(body) | GraphRequestOperation::PostJson { body, .. } => {
            client
                .post(request.url.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Arc::clone(body).to_vec())
        }
        GraphRequestOperation::PatchJson { body, .. } => client
            .patch(request.url.as_str())
            .header(CONTENT_TYPE, "application/json")
            .body(Arc::clone(body).to_vec()),
        GraphRequestOperation::DeleteCalendarEvent(_) => client.delete(request.url.as_str()),
    }
    .header(AUTHORIZATION, authorization);
    if let GraphRequestOperation::DeleteCalendarEvent(etag) = &request.operation {
        outbound = outbound.header(IF_MATCH, etag.0.clone());
    }
    if request.operation.requires_json_content_type() {
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
