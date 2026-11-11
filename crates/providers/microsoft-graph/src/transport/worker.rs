use std::{
    future::Future,
    sync::{Arc, mpsc as sync_mpsc},
    thread::{self, JoinHandle},
};

use huskarl::token::AccessToken;
use reqwest::{
    Client, Response, StatusCode,
    header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue, IF_MATCH},
};
use tokio::{
    runtime::{Builder, Runtime},
    sync::{mpsc, oneshot},
    task::JoinSet,
};

use crate::error::GraphError;

#[cfg(test)]
use super::client;
use super::{
    GraphCommand, GraphPatchOperation, GraphPostJsonOperation, GraphRequest, GraphRequestOperation,
    GraphResponse, GraphTransport, GraphTransportHandle, MAX_ACTIVE_REQUESTS,
    REQUEST_CHANNEL_CAPACITY, build_client,
    response::response_result,
    retry::{RetryDecision, rate_limit_error, retry_decision, wait_before_retry},
};

pub(super) fn start() -> Result<GraphTransport, GraphError> {
    WorkerStartup::new().launch(build_system_client)
}

#[cfg(test)]
pub(super) fn start_for_fixtures() -> Result<GraphTransport, GraphError> {
    WorkerStartup::new().launch(build_fixture_client)
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
    configured_client(build_client())
}

#[cfg(test)]
fn build_fixture_client() -> Result<Client, GraphError> {
    configured_client(client::build_fixture_client())
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

pub(super) fn authorization_header(token: &AccessToken) -> Result<HeaderValue, GraphError> {
    let mut header = token
        .expose_header_value()
        .map_err(|_| GraphError::InvalidToken)?;
    header.set_sensitive(true);
    Ok(header)
}

pub(super) async fn while_interested<T, F>(
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
