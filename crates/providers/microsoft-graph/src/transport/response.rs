use std::time::SystemTime;

use reqwest::{
    Response, StatusCode,
    header::{HeaderMap, LOCATION, RETRY_AFTER},
};
use tokio::sync::oneshot;

use crate::error::GraphError;

use super::retry::{rate_limit_error, retry_after_hint};
use super::{
    GraphCalendarDeltaOperation, GraphPatchOperation, GraphPostJsonOperation, GraphReadOperation,
    GraphRequestOperation, GraphResponse, MAX_GRAPH_CALENDAR_DELTA_URL_BYTES,
    MAX_RESPONSE_BODY_BYTES,
};

pub(super) async fn response_result(
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

pub(super) fn calendar_reset_location(headers: &HeaderMap) -> Result<Option<String>, GraphError> {
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
        match super::worker::while_interested(reply, response.chunk()).await? {
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

pub(super) fn append_response_chunk(body: &mut Vec<u8>, chunk: &[u8]) -> Result<(), GraphError> {
    if chunk.len() > MAX_RESPONSE_BODY_BYTES - body.len() {
        return Err(GraphError::ResponseTooLarge);
    }
    body.extend_from_slice(chunk);
    Ok(())
}
