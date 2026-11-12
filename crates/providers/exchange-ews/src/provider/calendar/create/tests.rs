use einfach_mail_application::CalendarOperationError;
use einfach_mail_domain::{
    AccountId, CalendarEventCreation, CalendarEventCreationId, CalendarEventDraft,
    CalendarOccurrenceTime, CalendarTimeZoneId, CalendarViewRange, CalendarViewReference, EventId,
};
use time::{Date, Month, PrimitiveDateTime, Time};

use crate::provider::calendar::item::parse_calendar_item_snapshot;

use super::{
    CreateItemResponseError, confirm_created_snapshot, create_item_request,
    parse_create_item_response,
};

const CREATE_SUCCESS: &[u8] = include_bytes!("../../../fixtures/create-item-success.xml");
const CREATE_REJECTED: &[u8] = include_bytes!("../../../fixtures/create-item-rejected.xml");
const CREATE_INCOMPLETE: &[u8] = include_bytes!("../../../fixtures/create-item-incomplete.xml");
const CREATE_MALFORMED: &[u8] = include_bytes!("../../../fixtures/create-item-malformed.xml");
const CREATED_CALENDAR_ITEM: &[u8] =
    include_bytes!("../../../fixtures/calendar-item-supported.xml");

fn creation(subject: &str) -> CalendarEventCreation {
    let date =
        Date::from_calendar_date(2026, Month::November, 12).expect("synthetic event date is valid");
    let next_date = Date::from_calendar_date(2026, Month::November, 13)
        .expect("synthetic event end date is valid");
    let start_time = Time::from_hms(10, 0, 0).expect("synthetic event start is valid");
    let end_time = Time::from_hms(11, 0, 0).expect("synthetic event end is valid");
    let local_start = PrimitiveDateTime::new(date, start_time);
    let local_end = PrimitiveDateTime::new(date, end_time);
    let start = local_start.assume_utc();
    let end = local_end.assume_utc();
    let draft = CalendarEventDraft::timed(
        subject.to_owned(),
        local_start,
        local_end,
        CalendarTimeZoneId::try_from("UTC".to_owned()).expect("synthetic time zone is valid"),
        start,
        end,
    )
    .expect("synthetic event draft is valid");
    let range = CalendarViewRange::new(
        date.midnight().assume_utc(),
        next_date.midnight().assume_utc(),
        date,
        next_date,
    )
    .expect("synthetic calendar range is valid");
    let view = CalendarViewReference::new(
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid"),
        range,
    );
    CalendarEventCreation::try_restore(
        CalendarEventCreationId::parse("00000000-0000-4000-8000-000000000401")
            .expect("synthetic operation ID is valid"),
        view,
        draft,
    )
    .expect("synthetic event fits the calendar view")
}

fn replace_once(document: &[u8], from: &str, to: &str) -> Vec<u8> {
    let document = String::from_utf8(document.to_vec()).expect("synthetic XML is UTF-8");
    assert_eq!(document.matches(from).count(), 1);
    document.replacen(from, to, 1).into_bytes()
}

#[test]
fn create_item_request_contains_only_supported_fields_and_the_stable_marker() {
    let request = create_item_request(&creation("Synthetic & <appointment>"))
        .expect("synthetic CreateItem request builds");
    let xml = String::from_utf8(request).expect("serialized request is UTF-8");

    assert!(xml.contains("SendMeetingInvitations=\"SendToNone\""));
    assert!(xml.contains("DistinguishedFolderId Id=\"calendar\""));
    assert!(xml.contains("<t:Subject>Synthetic &amp; &lt;appointment&gt;</t:Subject>"));
    assert!(
        xml.contains("<t:Start>2026-11-12T10:00:00.000000000Z</t:Start>"),
        "{xml}"
    );
    assert!(xml.contains("<t:End>2026-11-12T11:00:00.000000000Z</t:End>"));
    assert!(xml.contains("PropertySetId=\"4c2a2922-972e-54f9-bf4a-21e6b6d66d82\""));
    assert!(xml.contains("PropertyName=\"EinfachMailCalendarOperationId\""));
    assert!(xml.contains("PropertyType=\"String\""));
    assert!(xml.contains("00000000-0000-4000-8000-000000000401"));
    assert!(
        xml.find("</t:ExtendedProperty>").expect("marker closes")
            < xml.find("<t:Start>").expect("start serializes")
    );
    assert!(!xml.contains("<t:Body"));
    assert!(!xml.contains("<t:Location"));
    assert!(!xml.contains("Attendee"));
    assert!(!xml.contains("Recurrence"));
    assert!(!xml.contains("IsAllDayEvent"));
    assert!(!xml.contains("IsMeeting"));
    assert!(!xml.contains("TimeZoneContext"));
}

#[test]
fn create_item_response_accepts_one_successful_calendar_item_identity() {
    assert_eq!(
        parse_create_item_response(CREATE_SUCCESS),
        Ok(EventId::try_from("synthetic-calendar-event".to_owned())
            .expect("synthetic event ID is valid"))
    );
}

#[test]
fn create_item_response_rejects_only_an_explicit_matching_error() {
    assert_eq!(
        parse_create_item_response(CREATE_REJECTED),
        Err(CreateItemResponseError::Rejected)
    );

    let rejection_with_item = replace_once(
        CREATE_REJECTED,
        "<m:Items/>",
        concat!(
            "<m:Items><t:CalendarItem><t:ItemId Id=\"synthetic-calendar-event\" ",
            "ChangeKey=\"synthetic-change-key\"/></t:CalendarItem></m:Items>"
        ),
    );
    assert_eq!(
        parse_create_item_response(&rejection_with_item),
        Err(CreateItemResponseError::Unknown)
    );
}

#[test]
fn create_item_response_keeps_warnings_contradictions_and_extra_items_unknown() {
    let warning = replace_once(
        CREATE_SUCCESS,
        "ResponseClass=\"Success\"",
        "ResponseClass=\"Warning\"",
    );
    let contradiction = replace_once(
        CREATE_SUCCESS,
        "<m:ResponseCode>NoError",
        "<m:ResponseCode>ErrorAccessDenied",
    );
    let extra_item = replace_once(
        CREATE_SUCCESS,
        "</m:Items>",
        concat!(
            "<t:CalendarItem><t:ItemId Id=\"synthetic-duplicate\" ",
            "ChangeKey=\"synthetic-duplicate-key\"/></t:CalendarItem></m:Items>"
        ),
    );
    let extra_message = replace_once(
        CREATE_SUCCESS,
        "</m:ResponseMessages>",
        concat!(
            "<m:CreateItemResponseMessage ResponseClass=\"Success\">",
            "<m:ResponseCode>NoError</m:ResponseCode><m:Items/></m:CreateItemResponseMessage>",
            "</m:ResponseMessages>"
        ),
    );

    for response in [warning, contradiction, extra_item, extra_message] {
        assert_eq!(
            parse_create_item_response(&response),
            Err(CreateItemResponseError::Unknown)
        );
    }
}

#[test]
fn create_item_response_keeps_malformed_and_incomplete_data_unknown() {
    for response in [CREATE_MALFORMED, CREATE_INCOMPLETE] {
        assert_eq!(
            parse_create_item_response(response),
            Err(CreateItemResponseError::Unknown)
        );
    }

    let missing_code = replace_once(
        CREATE_SUCCESS,
        "<m:ResponseCode>NoError</m:ResponseCode>",
        "",
    );
    assert_eq!(
        parse_create_item_response(&missing_code),
        Err(CreateItemResponseError::Unknown)
    );
}

#[test]
fn create_item_response_requires_a_nonblank_id_and_change_key() {
    let blank_id = replace_once(
        CREATE_SUCCESS,
        "Id=\"synthetic-calendar-event\"",
        "Id=\" \"",
    );
    let blank_change_key = replace_once(
        CREATE_SUCCESS,
        "ChangeKey=\"synthetic-create-change-key\"",
        "ChangeKey=\" \"",
    );

    for response in [blank_id, blank_change_key] {
        assert_eq!(
            parse_create_item_response(&response),
            Err(CreateItemResponseError::Unknown)
        );
    }
}

#[test]
fn get_item_readback_must_match_the_persisted_creation_before_mapping() {
    let creation = creation("Synthetic calendar appointment");
    let snapshot = parse_calendar_item_snapshot(
        CREATED_CALENDAR_ITEM,
        &EventId::try_from("synthetic-calendar-event".to_owned())
            .expect("synthetic event ID is valid"),
    )
    .expect("synthetic created appointment parses");
    let event = confirm_created_snapshot(&creation, snapshot)
        .expect("synthetic read-back confirms the staged creation");

    assert_eq!(event.id().as_str(), "synthetic-calendar-event");
    assert_eq!(event.subject(), Some("Synthetic calendar appointment"));
    assert!(matches!(event.time(), CalendarOccurrenceTime::Timed { .. }));
}

#[test]
fn get_item_readback_keeps_a_different_creation_marker_unknown() {
    let creation = creation("Synthetic calendar appointment");
    let document = replace_once(
        CREATED_CALENDAR_ITEM,
        "00000000-0000-4000-8000-000000000401",
        "00000000-0000-4000-8000-000000000402",
    );
    let snapshot = parse_calendar_item_snapshot(
        &document,
        &EventId::try_from("synthetic-calendar-event".to_owned())
            .expect("synthetic event ID is valid"),
    )
    .expect("synthetic appointment parses");

    assert_eq!(
        confirm_created_snapshot(&creation, snapshot),
        Err(CalendarOperationError::CreationOutcomeUnknown)
    );
}
