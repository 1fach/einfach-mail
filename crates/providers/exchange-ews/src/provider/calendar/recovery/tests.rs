use einfach_mail_domain::{
    AccountId, CalendarEventCreation, CalendarEventCreationId, CalendarEventDraft,
    CalendarTimeZoneId, CalendarViewRange, CalendarViewReference, EventId,
};
use quick_xml::{NsReader, events::Event, name::ResolveResult};
use time::{Date, Month, PrimitiveDateTime, Time};

use super::{find_item_request, parse_find_item_candidate};
use crate::error::EwsError;

const FIND_SINGLE: &[u8] = include_bytes!("../../../fixtures/find-item-single.xml");
const FIND_ZERO: &[u8] = include_bytes!("../../../fixtures/find-item-zero.xml");
const FIND_MULTIPLE: &[u8] = include_bytes!("../../../fixtures/find-item-multiple.xml");
const FIND_INCOMPLETE: &[u8] = include_bytes!("../../../fixtures/find-item-incomplete.xml");
const FIND_REJECTED: &[u8] = include_bytes!("../../../fixtures/find-item-rejected.xml");
const FIND_MALFORMED: &[u8] = include_bytes!("../../../fixtures/find-item-malformed.xml");

fn creation() -> CalendarEventCreation {
    let date =
        Date::from_calendar_date(2026, Month::November, 12).expect("synthetic event date is valid");
    let next_date = Date::from_calendar_date(2026, Month::November, 13)
        .expect("synthetic event end date is valid");
    let local_start = PrimitiveDateTime::new(
        date,
        Time::from_hms(10, 0, 0).expect("synthetic event start is valid"),
    );
    let local_end = PrimitiveDateTime::new(
        date,
        Time::from_hms(11, 0, 0).expect("synthetic event end is valid"),
    );
    let draft = CalendarEventDraft::timed(
        "Synthetic calendar appointment".to_owned(),
        local_start,
        local_end,
        CalendarTimeZoneId::try_from("UTC".to_owned()).expect("synthetic time zone is valid"),
        local_start.assume_utc(),
        local_end.assume_utc(),
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

#[test]
fn recovery_find_item_request_is_a_bounded_exact_primary_calendar_marker_search() {
    let request = find_item_request(&creation()).expect("synthetic recovery request serializes");
    let xml = String::from_utf8(request).expect("serialized request is UTF-8");

    assert!(xml.contains("<FindItem"), "{xml}");
    assert!(xml.contains("Traversal=\"Shallow\""));
    assert!(xml.contains("MaxEntriesReturned=\"2\""));
    assert!(xml.contains("BasePoint=\"Beginning\""));
    assert!(xml.contains("<Restriction>"));
    assert!(xml.contains("<t:IsEqualTo>"));
    assert!(xml.contains("PropertySetId=\"4c2a2922-972e-54f9-bf4a-21e6b6d66d82\""));
    assert!(xml.contains("PropertyName=\"EinfachMailCalendarOperationId\""));
    assert!(xml.contains("PropertyType=\"String\""));
    assert!(xml.contains("Value=\"00000000-0000-4000-8000-000000000401\""));
    assert!(xml.contains("DistinguishedFolderId Id=\"calendar\""));
    assert!(!xml.contains("CreateItem"));

    let mut reader = NsReader::from_reader(xml.as_bytes());
    loop {
        let (namespace, event) = reader
            .read_resolved_event()
            .expect("generated request is well-formed XML");
        match event {
            Event::Start(element) if element.local_name().as_ref() == b"Restriction" => {
                assert!(
                    matches!(
                        namespace,
                        ResolveResult::Bound(uri)
                            if uri.as_ref() == b"http://schemas.microsoft.com/exchange/services/2006/messages"
                    ),
                    "{xml}"
                );
                break;
            }
            Event::Eof => panic!("generated request has no restriction element"),
            _ => {}
        }
    }
}

#[test]
fn complete_recovery_search_returns_one_calendar_item_candidate() {
    assert_eq!(
        parse_find_item_candidate(FIND_SINGLE),
        Ok(EventId::try_from("synthetic-calendar-event".to_owned())
            .expect("synthetic event ID is valid"))
    );
}

#[test]
fn recovery_search_keeps_zero_multiple_incomplete_and_error_results_unresolved() {
    for response in [
        FIND_ZERO,
        FIND_MULTIPLE,
        FIND_INCOMPLETE,
        FIND_REJECTED,
        FIND_MALFORMED,
    ] {
        assert_eq!(
            parse_find_item_candidate(response),
            Err(EwsError::InvalidResponse)
        );
    }
}

#[test]
fn recovery_search_rejects_non_calendar_items_and_extra_messages() {
    let non_calendar_item = String::from_utf8(FIND_SINGLE.to_vec())
        .expect("synthetic XML is UTF-8")
        .replace("CalendarItem", "Message");
    assert_eq!(
        parse_find_item_candidate(non_calendar_item.as_bytes()),
        Err(EwsError::InvalidResponse)
    );

    let extra_message = String::from_utf8(FIND_SINGLE.to_vec())
        .expect("synthetic XML is UTF-8")
        .replace(
            "</m:ResponseMessages>",
            concat!(
                "<m:FindItemResponseMessage ResponseClass=\"Success\">",
                "<m:ResponseCode>NoError</m:ResponseCode></m:FindItemResponseMessage>",
                "</m:ResponseMessages>"
            ),
        );
    assert_eq!(
        parse_find_item_candidate(extra_message.as_bytes()),
        Err(EwsError::InvalidResponse)
    );
}

#[test]
fn recovery_search_rejects_inconsistent_total_item_counts() {
    let response = String::from_utf8(FIND_SINGLE.to_vec())
        .expect("synthetic XML is UTF-8")
        .replace("TotalItemsInView=\"1\"", "TotalItemsInView=\"2\"");

    assert_eq!(
        parse_find_item_candidate(response.as_bytes()),
        Err(EwsError::InvalidResponse)
    );
}
