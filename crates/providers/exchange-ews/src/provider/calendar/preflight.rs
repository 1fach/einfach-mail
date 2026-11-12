use einfach_mail_application::CalendarOperationError;
use einfach_mail_domain::VersionedCalendarEvent;
use ews::response::ResponseCode;

use super::item::{CalendarItemSnapshot, CalendarItemSnapshotError, snapshot_matches_occurrence};
use super::map_access_denied_response_code;

pub(super) fn validate_write_snapshot(
    event: &VersionedCalendarEvent,
    snapshot: &CalendarItemSnapshot,
) -> Result<(), CalendarOperationError> {
    if snapshot.change_key == event.version().as_str()
        && snapshot_matches_occurrence(snapshot, event.occurrence())
    {
        Ok(())
    } else {
        Err(CalendarOperationError::EventChanged)
    }
}

pub(super) fn map_write_preflight_error(
    error: CalendarItemSnapshotError,
) -> CalendarOperationError {
    match error {
        CalendarItemSnapshotError::Ineligible => CalendarOperationError::EventChanged,
        CalendarItemSnapshotError::InvalidResponse => CalendarOperationError::InvalidResponse,
        CalendarItemSnapshotError::Rejected(code) => {
            map_item_read_rejection(code, CalendarOperationError::EventChanged)
        }
    }
}

pub(super) fn map_item_read_rejection(
    code: ResponseCode,
    missing_item: CalendarOperationError,
) -> CalendarOperationError {
    match code {
        ResponseCode::ErrorItemNotFound
        | ResponseCode::ErrorIrresolvableConflict
        | ResponseCode::ErrorStaleObject => missing_item,
        _ => map_access_denied_response_code(code)
            .map_or(CalendarOperationError::InvalidResponse, Into::into),
    }
}
