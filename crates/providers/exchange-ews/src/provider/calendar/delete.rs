use einfach_mail_application::CalendarOperationError;
use einfach_mail_domain::VersionedCalendarEvent;
use ews::response::ResponseCode;

use super::{
    item::{CalendarItemSnapshot, get_item_response, parse_calendar_item_snapshot},
    map_access_denied_response_code,
    preflight::{map_write_preflight_error, validate_write_snapshot},
};
use crate::transport::{EwsRequestSequence, EwsWriteError};

mod request;
mod response;
#[cfg(test)]
mod tests;

use request::delete_item_request;
use response::{DeleteItemResponseError, parse_delete_item_response};

pub(super) async fn delete_event(
    sequence: &EwsRequestSequence<'_>,
    event: &VersionedCalendarEvent,
) -> Result<(), CalendarOperationError> {
    let current = load_delete_source(sequence, event).await?;
    execute_delete_item(sequence, &current).await
}

async fn load_delete_source(
    sequence: &EwsRequestSequence<'_>,
    event: &VersionedCalendarEvent,
) -> Result<CalendarItemSnapshot, CalendarOperationError> {
    let response = get_item_response(sequence, event.occurrence().id())
        .await
        .map_err(CalendarOperationError::from)?;
    let current = parse_calendar_item_snapshot(&response, event.occurrence().id())
        .map_err(map_write_preflight_error)?;
    validate_write_snapshot(event, &current)?;
    Ok(current)
}

async fn execute_delete_item(
    sequence: &EwsRequestSequence<'_>,
    current: &CalendarItemSnapshot,
) -> Result<(), CalendarOperationError> {
    let request = delete_item_request(current).map_err(CalendarOperationError::from)?;
    let response = match sequence.execute_xml_write(&request).await {
        Ok(response) => response,
        Err(EwsWriteError::NotDispatched(error)) => return Err(error.into()),
        Err(EwsWriteError::OutcomeUnknown) => {
            return Err(CalendarOperationError::DeleteOutcomeUnknown);
        }
    };
    parse_delete_item_response(&response).map_err(map_delete_response_error)
}

fn map_delete_response_error(error: DeleteItemResponseError) -> CalendarOperationError {
    match error {
        DeleteItemResponseError::Rejected(code) => match code {
            ResponseCode::ErrorIrresolvableConflict
            | ResponseCode::ErrorItemNotFound
            | ResponseCode::ErrorStaleObject => CalendarOperationError::EventChanged,
            _ => map_access_denied_response_code(code)
                .map_or(CalendarOperationError::EventDeletionRejected, Into::into),
        },
        DeleteItemResponseError::Unknown => CalendarOperationError::DeleteOutcomeUnknown,
    }
}
