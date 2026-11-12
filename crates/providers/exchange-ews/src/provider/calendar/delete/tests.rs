use einfach_mail_application::CalendarOperationError;
use einfach_mail_domain::{
    AccountId, CalendarEventVersion, CalendarOccurrence, CalendarOccurrencePatch,
    CalendarOccurrenceTimePatch, CalendarViewRange, CalendarViewReference, EventId,
    VersionedCalendarEvent,
};
use ews::response::ResponseCode;
use time::{Date, Month, OffsetDateTime, UtcOffset, format_description::well_known::Iso8601};

use crate::provider::calendar::item::{CalendarItemSnapshotError, parse_calendar_item_snapshot};
use crate::provider::calendar::preflight::map_write_preflight_error;

use super::super::preflight::validate_write_snapshot;
use super::{
    DeleteItemResponseError, delete_item_request, map_delete_response_error,
    parse_delete_item_response,
};

const ITEM: &[u8] = include_bytes!("../../../fixtures/calendar-item-supported.xml");
const WRONG_KIND_ITEM: &[u8] = include_bytes!("../../../fixtures/calendar-item-wrong-kind.xml");
const MALFORMED_ITEM: &[u8] = include_bytes!("../../../fixtures/calendar-item-malformed.xml");
const INCOMPLETE_ITEM: &[u8] = include_bytes!("../../../fixtures/calendar-item-incomplete.xml");
const DELETE_SUCCESS: &[u8] = include_bytes!("../../../fixtures/calendar-delete-item-success.xml");
const DELETE_REJECTED: &[u8] =
    include_bytes!("../../../fixtures/calendar-delete-item-rejected.xml");
const DELETE_CONFLICT: &[u8] =
    include_bytes!("../../../fixtures/calendar-delete-item-conflict.xml");
const DELETE_MALFORMED: &[u8] =
    include_bytes!("../../../fixtures/calendar-delete-item-malformed.xml");
const DELETE_INCOMPLETE: &[u8] =
    include_bytes!("../../../fixtures/calendar-delete-item-incomplete.xml");
const DELETE_EXTRA_ITEMS: &[u8] =
    include_bytes!("../../../fixtures/calendar-delete-item-extra-items.xml");

const EVENT_ID: &str = "synthetic-calendar-event";
const CHANGE_KEY: &str = "synthetic-change-key";
const SUBJECT: &str = "Synthetic calendar appointment";
const START: &str = "2026-11-12T10:00:00Z";
const END: &str = "2026-11-12T11:00:00Z";

#[test]
fn delete_request_moves_one_versioned_item_without_notifying_attendees() {
    let snapshot = parse_calendar_item_snapshot(ITEM, &event_id())
        .expect("synthetic standalone appointment is eligible");
    let document = delete_item_request(&snapshot).expect("synthetic delete request builds");
    let xml = String::from_utf8(document).expect("serialized EWS request is UTF-8");

    assert!(xml.contains("DeleteType=\"MoveToDeletedItems\""));
    assert!(xml.contains("SendMeetingCancellations=\"SendToNone\""));
    assert!(xml.contains("Id=\"synthetic-calendar-event\""));
    assert!(xml.contains("ChangeKey=\"synthetic-change-key\""));
    assert!(!xml.contains("DeleteType=\"HardDelete\""));
    assert!(!xml.contains("DeleteType=\"SoftDelete\""));
    assert!(!xml.contains("SendMeetingCancellations=\"SendToAll"));
}

#[test]
fn delete_preflight_requires_the_same_version_and_occurrence_values() {
    let snapshot = parse_calendar_item_snapshot(ITEM, &event_id())
        .expect("synthetic standalone appointment is eligible");
    let current = versioned_event(CHANGE_KEY);
    let stale = versioned_event("synthetic-stale-change-key");

    assert_eq!(validate_write_snapshot(&current, &snapshot), Ok(()));
    assert_eq!(
        validate_write_snapshot(&stale, &snapshot),
        Err(CalendarOperationError::EventChanged)
    );
}

#[test]
fn delete_preflight_rejects_unsupported_and_malformed_get_item_snapshots() {
    let meeting = replace_once(
        ITEM,
        "<t:IsMeeting>false</t:IsMeeting>",
        "<t:IsMeeting>true</t:IsMeeting>",
    );

    for response in [meeting.as_slice(), WRONG_KIND_ITEM] {
        assert_eq!(
            parse_calendar_item_snapshot(response, &event_id()),
            Err(CalendarItemSnapshotError::Ineligible)
        );
        assert_eq!(
            map_write_preflight_error(CalendarItemSnapshotError::Ineligible),
            CalendarOperationError::EventChanged
        );
    }

    for response in [MALFORMED_ITEM, INCOMPLETE_ITEM] {
        assert_eq!(
            parse_calendar_item_snapshot(response, &event_id()),
            Err(CalendarItemSnapshotError::InvalidResponse)
        );
        assert_eq!(
            map_write_preflight_error(CalendarItemSnapshotError::InvalidResponse),
            CalendarOperationError::InvalidResponse
        );
    }
}

#[test]
fn delete_response_requires_one_exact_success_and_maps_known_rejections() {
    assert!(
        super::super::validate_calendar_operation_response(
            DELETE_SUCCESS,
            b"DeleteItemResponse",
            b"DeleteItemResponseMessage",
        )
        .is_ok()
    );
    assert_eq!(parse_delete_item_response(DELETE_SUCCESS), Ok(()));
    assert_eq!(
        parse_delete_item_response(DELETE_REJECTED),
        Err(DeleteItemResponseError::Rejected(
            ResponseCode::ErrorItemSave
        ))
    );
    assert_eq!(
        map_delete_response_error(DeleteItemResponseError::Rejected(
            ResponseCode::ErrorItemSave
        )),
        CalendarOperationError::EventDeletionRejected
    );
    let access_denied = replace_once(DELETE_REJECTED, "ErrorItemSave", "ErrorAccessDenied");
    assert_eq!(
        parse_delete_item_response(&access_denied),
        Err(DeleteItemResponseError::Rejected(
            ResponseCode::ErrorAccessDenied
        ))
    );
    assert_eq!(
        map_delete_response_error(DeleteItemResponseError::Rejected(
            ResponseCode::ErrorAccessDenied
        )),
        CalendarOperationError::AccessDenied
    );
    assert_eq!(
        parse_delete_item_response(DELETE_CONFLICT),
        Err(DeleteItemResponseError::Rejected(
            ResponseCode::ErrorStaleObject
        ))
    );
    assert_eq!(
        map_delete_response_error(DeleteItemResponseError::Rejected(
            ResponseCode::ErrorStaleObject
        )),
        CalendarOperationError::EventChanged
    );
}

#[test]
fn delete_response_keeps_ambiguous_or_incomplete_results_unknown() {
    let warning = replace_once(
        DELETE_SUCCESS,
        "ResponseClass=\"Success\"",
        "ResponseClass=\"Warning\"",
    );
    let error_with_no_error = replace_once(
        DELETE_SUCCESS,
        "ResponseClass=\"Success\"",
        "ResponseClass=\"Error\"",
    );
    let success_with_error = replace_once(
        DELETE_SUCCESS,
        "<m:ResponseCode>NoError</m:ResponseCode>",
        "<m:ResponseCode>ErrorItemSave</m:ResponseCode>",
    );
    let duplicate_code = replace_once(
        DELETE_SUCCESS,
        "</m:ResponseCode>",
        "</m:ResponseCode><m:ResponseCode>NoError</m:ResponseCode>",
    );
    let extra_message = replace_once(
        DELETE_SUCCESS,
        "</m:ResponseMessages>",
        "<m:DeleteItemResponseMessage ResponseClass=\"Success\"><m:ResponseCode>NoError</m:ResponseCode></m:DeleteItemResponseMessage></m:ResponseMessages>",
    );

    for response in [
        warning,
        error_with_no_error,
        success_with_error,
        duplicate_code,
        extra_message,
        DELETE_MALFORMED.to_vec(),
        DELETE_INCOMPLETE.to_vec(),
        DELETE_EXTRA_ITEMS.to_vec(),
    ] {
        assert_eq!(
            parse_delete_item_response(&response),
            Err(DeleteItemResponseError::Unknown)
        );
    }
    assert_eq!(
        map_delete_response_error(DeleteItemResponseError::Unknown),
        CalendarOperationError::DeleteOutcomeUnknown
    );
}

fn versioned_event(change_key: &str) -> VersionedCalendarEvent {
    VersionedCalendarEvent::new(
        event(),
        CalendarEventVersion::try_from(change_key.to_owned())
            .expect("synthetic EWS change key is valid"),
    )
    .expect("synthetic timed occurrence is versioned")
}

fn event() -> CalendarOccurrence {
    let mut time = CalendarOccurrenceTimePatch::new();
    time.set_start(timestamp(START));
    time.set_end(timestamp(END));
    time.set_is_all_day(false);
    let mut patch = CalendarOccurrencePatch::new();
    patch.set_subject(Some(SUBJECT.to_owned()));
    patch.set_time(time);
    patch
        .into_new(view(), event_id())
        .expect("synthetic calendar occurrence is valid")
}

fn view() -> CalendarViewReference {
    let start = timestamp("2026-11-01T00:00:00Z");
    let end = timestamp("2026-12-01T00:00:00Z");
    let range = CalendarViewRange::new(
        start,
        end,
        Date::from_calendar_date(2026, Month::November, 1)
            .expect("synthetic view start date is valid"),
        Date::from_calendar_date(2026, Month::December, 1)
            .expect("synthetic view end date is valid"),
    )
    .expect("synthetic calendar view range is valid");
    let account_id = AccountId::try_from("synthetic-ews-account".to_owned())
        .expect("synthetic account ID is valid");
    CalendarViewReference::new(account_id, range)
}

fn event_id() -> EventId {
    EventId::try_from(EVENT_ID.to_owned()).expect("synthetic event ID is valid")
}

fn timestamp(value: &str) -> OffsetDateTime {
    OffsetDateTime::parse(value, &Iso8601::DEFAULT)
        .expect("synthetic UTC timestamp is valid")
        .to_offset(UtcOffset::UTC)
}

fn replace_once(document: &[u8], from: &str, to: &str) -> Vec<u8> {
    let document = String::from_utf8(document.to_vec()).expect("synthetic XML is UTF-8");
    assert_eq!(document.matches(from).count(), 1);
    document.replacen(from, to, 1).into_bytes()
}
