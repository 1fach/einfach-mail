use einfach_mail_application::CalendarOperationError;
use einfach_mail_domain::{
    AccountId, CalendarEventUpdate, CalendarEventUpdateDraft, CalendarEventVersion,
    CalendarOccurrence, CalendarOccurrencePatch, CalendarOccurrenceTimePatch, CalendarViewRange,
    CalendarViewReference, EventId, VersionedCalendarEvent,
};
use ews::response::ResponseCode;
use time::{Date, Month, OffsetDateTime, UtcOffset, format_description::well_known::Iso8601};

use crate::provider::calendar::item::parse_calendar_item_snapshot;

use super::{
    UpdateItemResponseError, confirm_updated_snapshot, map_update_response_error,
    parse_update_item_response, update_item_request,
};

const ITEM: &[u8] = include_bytes!("../../../fixtures/calendar-item-supported.xml");
const UPDATE_SUCCESS: &[u8] = include_bytes!("../../../fixtures/calendar-update-item-success.xml");
const UPDATE_NOOP_SUCCESS: &[u8] =
    include_bytes!("../../../fixtures/calendar-update-item-noop-success.xml");
const UPDATE_REJECTED: &[u8] =
    include_bytes!("../../../fixtures/calendar-update-item-rejected.xml");
const UPDATE_CONFLICT: &[u8] =
    include_bytes!("../../../fixtures/calendar-update-item-conflict.xml");
const UPDATE_MALFORMED: &[u8] =
    include_bytes!("../../../fixtures/calendar-update-item-malformed.xml");
const UPDATE_INCOMPLETE: &[u8] =
    include_bytes!("../../../fixtures/calendar-update-item-incomplete.xml");

const EVENT_ID: &str = "synthetic-calendar-event";
const CHANGE_KEY: &str = "synthetic-change-key";
const SUBJECT: &str = "Synthetic calendar appointment";
const START: &str = "2026-11-12T10:00:00Z";
const END: &str = "2026-11-12T11:00:00Z";

#[test]
fn update_request_sends_only_the_changed_subject_with_safe_attributes() {
    let update = event_update("Updated & <synthetic> appointment", START, END);
    let document =
        update_item_request(&update, CHANGE_KEY).expect("synthetic UpdateItem request builds");
    let xml = String::from_utf8(document).expect("serialized EWS request is UTF-8");

    assert!(xml.contains("ConflictResolution=\"NeverOverwrite\""));
    assert!(xml.contains("SendMeetingInvitationsOrCancellations=\"SendToNone\""));
    assert!(xml.contains("Id=\"synthetic-calendar-event\""));
    assert!(xml.contains("ChangeKey=\"synthetic-change-key\""));
    assert!(xml.contains("FieldURI=\"item:Subject\""));
    assert!(xml.contains("<t:CalendarItem><t:Subject>Updated &amp; &lt;synthetic&gt; appointment</t:Subject></t:CalendarItem>"));
    assert!(!xml.contains("FieldURI=\"calendar:Start\""));
    assert!(!xml.contains("FieldURI=\"calendar:End\""));
    assert!(!xml.contains("<t:Body"));
    assert!(!xml.contains("<t:Location"));
    assert!(!xml.contains("Attendee"));
    assert!(!xml.contains("Recurrence"));
}

#[test]
fn update_request_sends_both_utc_endpoints_when_one_changes() {
    let update = event_update(SUBJECT, "2026-11-12T10:30:00Z", END);
    let document = update_item_request(&update, CHANGE_KEY)
        .expect("synthetic timed UpdateItem request builds");
    let xml = String::from_utf8(document).expect("serialized EWS request is UTF-8");

    assert!(xml.contains("FieldURI=\"calendar:Start\""));
    assert!(xml.contains("FieldURI=\"calendar:End\""));
    assert!(xml.contains("<t:Start>2026-11-12T10:30:00.000000000Z</t:Start>"));
    assert!(xml.contains("<t:End>2026-11-12T11:00:00.000000000Z</t:End>"));
    assert!(!xml.contains("FieldURI=\"item:Subject\""));
}

#[test]
fn update_response_accepts_exact_success_and_a_change_key_noop() {
    assert_eq!(
        parse_update_item_response(UPDATE_SUCCESS, &event_id())
            .expect("synthetic update success has an identity")
            .as_str(),
        "synthetic-updated-change-key"
    );
    assert_eq!(
        parse_update_item_response(UPDATE_NOOP_SUCCESS, &event_id())
            .expect("a successful no-op may retain its change key")
            .as_str(),
        CHANGE_KEY
    );
}

#[test]
fn no_op_update_confirmation_accepts_readback_with_the_same_change_key() {
    let update = event_update("Updated calendar appointment", START, END);
    let document = String::from_utf8_lossy(ITEM).replace(SUBJECT, "Updated calendar appointment");
    let snapshot = parse_calendar_item_snapshot(document.as_bytes(), &event_id())
        .expect("synthetic GetItem readback matches the update");
    let response_version = parse_update_item_response(UPDATE_NOOP_SUCCESS, &event_id())
        .expect("synthetic UpdateItem response retains the current change key");

    let confirmed = confirm_updated_snapshot(&update, &response_version, snapshot)
        .expect("readback accepts the requested values and unchanged key");

    assert!(update.confirms(&confirmed));
    assert_eq!(confirmed.subject(), Some("Updated calendar appointment"));
}

#[test]
fn updated_item_readback_confirms_requested_values_and_new_change_key() {
    let update = event_update(
        "Updated calendar appointment",
        "2026-11-12T10:30:00Z",
        "2026-11-12T11:30:00Z",
    );
    let document = String::from_utf8_lossy(ITEM)
        .replace(SUBJECT, "Updated calendar appointment")
        .replace(START, "2026-11-12T10:30:00Z")
        .replace(END, "2026-11-12T11:30:00Z")
        .replace(CHANGE_KEY, "synthetic-updated-change-key");
    let snapshot = parse_calendar_item_snapshot(document.as_bytes(), &event_id())
        .expect("synthetic GetItem readback matches the updated item");
    let response_version = parse_update_item_response(UPDATE_SUCCESS, &event_id())
        .expect("synthetic UpdateItem response provides the updated version");

    let confirmed = confirm_updated_snapshot(&update, &response_version, snapshot)
        .expect("readback confirms the requested values and response version");

    assert!(update.confirms(&confirmed));
    assert_eq!(confirmed.subject(), Some("Updated calendar appointment"));
    assert_eq!(confirmed.time(), update.draft().time());
}

#[test]
fn update_response_distinguishes_explicit_rejection_from_conflict() {
    assert_eq!(
        parse_update_item_response(UPDATE_REJECTED, &event_id()),
        Err(UpdateItemResponseError::Rejected(
            ResponseCode::ErrorItemSave
        ))
    );
    assert_eq!(
        map_update_response_error(UpdateItemResponseError::Rejected(
            ResponseCode::ErrorItemSave
        )),
        CalendarOperationError::EventUpdateRejected
    );
    assert_eq!(
        map_update_response_error(UpdateItemResponseError::Rejected(
            ResponseCode::ErrorIrresolvableConflict
        )),
        CalendarOperationError::EventChanged
    );
    assert_eq!(
        parse_update_item_response(UPDATE_CONFLICT, &event_id()),
        Err(UpdateItemResponseError::Rejected(
            ResponseCode::ErrorIrresolvableConflict
        ))
    );
}

#[test]
fn update_response_keeps_incomplete_contradictory_and_malformed_results_unknown() {
    let warning = replace_once(
        UPDATE_SUCCESS,
        "ResponseClass=\"Success\"",
        "ResponseClass=\"Warning\"",
    );
    let contradiction = replace_once(
        UPDATE_SUCCESS,
        "<m:ResponseCode>NoError</m:ResponseCode>",
        "<m:ResponseCode>ErrorItemSave</m:ResponseCode>",
    );
    let missing_item = replace_once(
        UPDATE_SUCCESS,
        concat!(
            "            <t:CalendarItem>\n",
            "              <t:ItemId Id=\"synthetic-calendar-event\" ChangeKey=\"synthetic-updated-change-key\"/>\n",
            "            </t:CalendarItem>\n"
        ),
        "",
    );
    let wrong_id = replace_once(UPDATE_SUCCESS, EVENT_ID, "synthetic-other-event");
    let empty_change_key = replace_once(
        UPDATE_SUCCESS,
        "ChangeKey=\"synthetic-updated-change-key\"",
        "ChangeKey=\" \"",
    );
    let extra_item = replace_once(
        UPDATE_SUCCESS,
        "</m:Items>",
        "            <t:CalendarItem><t:ItemId Id=\"synthetic-extra-event\" ChangeKey=\"synthetic-extra-key\"/></t:CalendarItem></m:Items>",
    );
    let extra_message = replace_once(
        UPDATE_SUCCESS,
        "</m:ResponseMessages>",
        "        <m:UpdateItemResponseMessage ResponseClass=\"Success\"><m:ResponseCode>NoError</m:ResponseCode><m:Items/></m:UpdateItemResponseMessage></m:ResponseMessages>",
    );

    for response in [
        warning,
        contradiction,
        missing_item,
        wrong_id,
        empty_change_key,
        extra_item,
        extra_message,
        UPDATE_INCOMPLETE.to_vec(),
        UPDATE_MALFORMED.to_vec(),
    ] {
        assert_eq!(
            parse_update_item_response(&response, &event_id()),
            Err(UpdateItemResponseError::Unknown)
        );
    }
}

#[test]
fn explicit_rejections_with_items_are_unknown() {
    let response = String::from_utf8_lossy(UPDATE_REJECTED).replace(
        "<m:Items/>",
        concat!(
            "<m:Items><t:CalendarItem><t:ItemId Id=\"synthetic-calendar-event\" ",
            "ChangeKey=\"synthetic-change-key\"/></t:CalendarItem></m:Items>"
        ),
    );

    assert_eq!(
        parse_update_item_response(response.as_bytes(), &event_id()),
        Err(UpdateItemResponseError::Unknown)
    );
}

fn event_update(subject: &str, start: &str, end: &str) -> CalendarEventUpdate {
    let source = VersionedCalendarEvent::new(
        event(),
        CalendarEventVersion::try_from(CHANGE_KEY.to_owned())
            .expect("synthetic EWS change key is a valid version"),
    )
    .expect("synthetic timed occurrence is versioned");
    let draft =
        CalendarEventUpdateDraft::timed(subject.to_owned(), timestamp(start), timestamp(end))
            .expect("synthetic EWS update draft is valid");
    CalendarEventUpdate::new(source, draft).expect("synthetic update changes event values")
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
    let range_start = timestamp("2026-11-01T00:00:00Z");
    let range_end = timestamp("2026-12-01T00:00:00Z");
    let start_date = Date::from_calendar_date(2026, Month::November, 1)
        .expect("synthetic view start date is valid");
    let end_date = Date::from_calendar_date(2026, Month::December, 1)
        .expect("synthetic view end date is valid");
    let range = CalendarViewRange::new(range_start, range_end, start_date, end_date)
        .expect("synthetic calendar view is valid");
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
