use std::{sync::Arc, thread::JoinHandle};

use huskarl::token::AccessToken;
use reqwest::header::HeaderValue;
#[cfg(test)]
use reqwest::header::LOCATION;
use tokio::sync::{mpsc, oneshot};

use crate::error::GraphError;

#[cfg(test)]
mod tests;

mod client;
mod response;
mod retry;
mod url;
mod worker;

pub(super) use self::client::build_client;
#[cfg(test)]
pub(super) use self::url::CALENDAR_EVENT_EDIT_SELECT;
pub(super) use self::url::{GraphAuthority, GraphUrl, MAX_GRAPH_CALENDAR_DELTA_URL_BYTES};

const REQUEST_CHANNEL_CAPACITY: usize = 16;
const MAX_ACTIVE_REQUESTS: usize = 16;
const MAX_RESPONSE_BODY_BYTES: usize = 8 * 1024 * 1024;

#[cfg(test)]
use self::{
    response::{append_response_chunk, calendar_reset_location},
    retry::{
        RetryAfter, RetryDecision, fallback_delay, parse_retry_after, retry_after_hint,
        retry_decision_for,
    },
    worker::authorization_header,
};

#[derive(Clone, Copy)]
pub(super) enum GraphReadOperation {
    Profile,
    Folders,
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
        match self {
            Self::Get(
                GraphReadOperation::Profile
                | GraphReadOperation::Folders
                | GraphReadOperation::MessageBody
                | GraphReadOperation::CalendarEventRead,
            )
            | Self::SendMail(_)
            | Self::PostJson { .. }
            | Self::PatchJson { .. }
            | Self::DeleteCalendarEvent(_) => false,
            Self::MessageDelta | Self::CalendarViewDelta(_) => true,
        }
    }

    fn uses_immutable_ids(&self) -> bool {
        match self {
            Self::Get(GraphReadOperation::Profile | GraphReadOperation::Folders)
            | Self::SendMail(_) => false,
            Self::Get(GraphReadOperation::MessageBody | GraphReadOperation::CalendarEventRead)
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
        worker::start()
    }

    #[cfg(test)]
    pub(super) fn start_for_fixtures() -> Result<Self, GraphError> {
        worker::start_for_fixtures()
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
