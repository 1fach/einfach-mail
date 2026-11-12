use einfach_mail_application::CalendarOperationError;
use einfach_mail_domain::{
    CalendarEventUpdate, CalendarEventVersion, CalendarOccurrence, CalendarOccurrencePatch,
    CalendarOccurrenceTime, CalendarOccurrenceTimePatch, VersionedCalendarEvent,
};

use super::{
    item::{
        CalendarItemSnapshot, CalendarItemSnapshotError, get_item_response,
        parse_calendar_item_snapshot,
    },
    preflight::{map_item_read_rejection, map_write_preflight_error, validate_write_snapshot},
};
use crate::transport::{EwsRequestSequence, EwsWriteError};

mod request;
mod response;
#[cfg(test)]
mod tests;

use request::update_item_request;
#[cfg(test)]
use response::UpdateItemResponseError;
use response::{map_update_response_error, parse_update_item_response};

pub(super) async fn load_versioned_event(
    sequence: &EwsRequestSequence<'_>,
    event: &CalendarOccurrence,
) -> Result<VersionedCalendarEvent, CalendarOperationError> {
    let response = get_item_response(sequence, event.id())
        .await
        .map_err(CalendarOperationError::from)?;
    let snapshot =
        parse_calendar_item_snapshot(&response, event.id()).map_err(map_load_snapshot_error)?;
    versioned_event(event, snapshot)
}

pub(super) async fn update_event(
    sequence: &EwsRequestSequence<'_>,
    update: &CalendarEventUpdate,
) -> Result<CalendarOccurrence, CalendarOperationError> {
    let current = load_update_source(sequence, update).await?;
    let response_version = execute_update_item(sequence, update, &current.change_key).await?;
    let snapshot = load_confirmed_snapshot(sequence, update).await?;
    confirm_updated_snapshot(update, &response_version, snapshot)
}

async fn load_update_source(
    sequence: &EwsRequestSequence<'_>,
    update: &CalendarEventUpdate,
) -> Result<CalendarItemSnapshot, CalendarOperationError> {
    let source = update.source();
    let event = source.occurrence();
    let response = get_item_response(sequence, event.id())
        .await
        .map_err(CalendarOperationError::from)?;
    let current =
        parse_calendar_item_snapshot(&response, event.id()).map_err(map_write_preflight_error)?;
    validate_write_snapshot(source, &current)?;
    Ok(current)
}

async fn execute_update_item(
    sequence: &EwsRequestSequence<'_>,
    update: &CalendarEventUpdate,
    change_key: &str,
) -> Result<CalendarEventVersion, CalendarOperationError> {
    let request = update_item_request(update, change_key).map_err(CalendarOperationError::from)?;
    let response = match sequence.execute_xml_write(&request).await {
        Ok(response) => response,
        Err(EwsWriteError::NotDispatched(error)) => return Err(error.into()),
        Err(EwsWriteError::OutcomeUnknown) => {
            return Err(CalendarOperationError::UpdateOutcomeUnknown);
        }
    };
    parse_update_item_response(&response, update.source().occurrence().id())
        .map_err(map_update_response_error)
}

async fn load_confirmed_snapshot(
    sequence: &EwsRequestSequence<'_>,
    update: &CalendarEventUpdate,
) -> Result<CalendarItemSnapshot, CalendarOperationError> {
    let event_id = update.source().occurrence().id();
    let response = get_item_response(sequence, event_id)
        .await
        .map_err(|_| CalendarOperationError::UpdateOutcomeUnknown)?;
    parse_calendar_item_snapshot(&response, event_id)
        .map_err(|_| CalendarOperationError::UpdateOutcomeUnknown)
}

fn map_load_snapshot_error(error: CalendarItemSnapshotError) -> CalendarOperationError {
    match error {
        CalendarItemSnapshotError::Ineligible => CalendarOperationError::EventNotEditable,
        CalendarItemSnapshotError::InvalidResponse => CalendarOperationError::InvalidResponse,
        CalendarItemSnapshotError::Rejected(code) => {
            map_item_read_rejection(code, CalendarOperationError::EventNotEditable)
        }
    }
}

fn versioned_event(
    event: &CalendarOccurrence,
    snapshot: CalendarItemSnapshot,
) -> Result<VersionedCalendarEvent, CalendarOperationError> {
    let version = CalendarEventVersion::try_from(snapshot.change_key.clone())
        .map_err(|_| CalendarOperationError::InvalidResponse)?;
    let occurrence = occurrence_with_snapshot(event, snapshot)?;
    VersionedCalendarEvent::new(occurrence, version)
        .map_err(|_| CalendarOperationError::InvalidResponse)
}

fn occurrence_with_snapshot(
    source: &CalendarOccurrence,
    snapshot: CalendarItemSnapshot,
) -> Result<CalendarOccurrence, CalendarOperationError> {
    let mut time = CalendarOccurrenceTimePatch::new();
    time.set_start(snapshot.start);
    time.set_end(snapshot.end);
    time.set_is_all_day(false);
    let mut patch = CalendarOccurrencePatch::new();
    patch.set_subject(Some(snapshot.subject));
    patch.set_time(time);
    let mut occurrence = source.clone();
    patch
        .apply_to(&mut occurrence)
        .map_err(|_| CalendarOperationError::InvalidResponse)?;
    Ok(occurrence)
}

fn confirm_updated_snapshot(
    update: &CalendarEventUpdate,
    response_version: &CalendarEventVersion,
    snapshot: CalendarItemSnapshot,
) -> Result<CalendarOccurrence, CalendarOperationError> {
    if !snapshot_matches_update(update, response_version, &snapshot) {
        return Err(CalendarOperationError::UpdateOutcomeUnknown);
    }
    let occurrence = occurrence_with_snapshot(update.source().occurrence(), snapshot)
        .map_err(|_| CalendarOperationError::UpdateOutcomeUnknown)?;
    if update.confirms(&occurrence) {
        Ok(occurrence)
    } else {
        Err(CalendarOperationError::UpdateOutcomeUnknown)
    }
}

fn snapshot_matches_update(
    update: &CalendarEventUpdate,
    response_version: &CalendarEventVersion,
    snapshot: &CalendarItemSnapshot,
) -> bool {
    let CalendarOccurrenceTime::Timed { start, end } = update.draft().time() else {
        return false;
    };
    snapshot.id == *update.source().occurrence().id()
        && snapshot.change_key == response_version.as_str()
        && snapshot.subject == update.draft().subject()
        && snapshot.start == *start
        && snapshot.end == *end
}
