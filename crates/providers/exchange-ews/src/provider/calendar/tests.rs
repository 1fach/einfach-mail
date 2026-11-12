use einfach_mail_domain::{
    AccountId, CalendarOccurrenceTime, CalendarViewRange, CalendarViewReference,
};
use time::{Date, Month, OffsetDateTime};

use crate::transport::serialize_soap_request;

use super::{
    CalendarItemElement, EwsCalendarItem, EwsError, MAX_CALENDAR_OCCURRENCES,
    build_calendar_view_request, map_snapshot_items, parse_calendar_view_snapshot,
};

const CALENDAR_VIEW_RESPONSE: &[u8] = include_bytes!("../..//fixtures/calendar-view-response.xml");
const EMPTY_CALENDAR_VIEW_RESPONSE: &[u8] =
    include_bytes!("../..//fixtures/empty-calendar-view-response.xml");

#[test]
fn request_uses_utc_calendar_view_bounds_and_only_required_fields() {
    let request = build_calendar_view_request(&calendar_view())
        .expect("synthetic calendar view builds a request");
    let document = serialize_soap_request(request).expect("request serializes as SOAP");
    let xml = String::from_utf8(document).expect("SOAP request is UTF-8");

    assert!(
        xml.contains("<CalendarView MaxEntriesReturned=\"512\" StartDate=\"2026-11-01T00:00:00.000000000Z\" EndDate=\"2026-11-08T00:00:00.000000000Z\"/>"),
        "{xml}"
    );
    assert!(xml.contains("<t:DistinguishedFolderId Id=\"calendar\"></t:DistinguishedFolderId>"));
    assert!(xml.contains("FieldURI=\"calendar:StartTimeZoneId\""));
    assert!(xml.contains("FieldURI=\"calendar:EndTimeZoneId\""));
    assert!(xml.contains("FieldURI=\"calendar:CalendarItemType\""));
    assert!(!xml.contains("TimeZoneContext"));
}

#[test]
fn maps_occurrences_exceptions_and_dst_crossing_all_day_events() {
    let response = parse_calendar_view_snapshot(
        CALENDAR_VIEW_RESPONSE,
        CALENDAR_VIEW_RESPONSE.len(),
        &calendar_view(),
    )
    .expect("synthetic EWS calendar snapshot parses");
    let (occurrences, response_bytes) = response.into_parts();

    assert_eq!(response_bytes, CALENDAR_VIEW_RESPONSE.len());
    assert_eq!(occurrences.len(), 3);
    assert_eq!(
        occurrences[0].id().as_str(),
        "synthetic-recurring-occurrence"
    );
    assert_eq!(
        occurrences[0].subject(),
        Some("Synthetic recurring occurrence")
    );
    assert_eq!(occurrences[0].location(), Some("Synthetic room"));
    assert_eq!(
        occurrences[0].original_start_time_zone(),
        Some("Eastern Standard Time")
    );
    assert!(matches!(
        occurrences[0].time(),
        CalendarOccurrenceTime::Timed { start, end }
            if start.unix_timestamp() == 1_793_511_000
                && end.unix_timestamp() == 1_793_514_600
    ));
    assert_eq!(
        occurrences[1].id().as_str(),
        "synthetic-recurring-exception"
    );
    assert_eq!(
        occurrences[1].subject(),
        Some("Synthetic recurring exception")
    );
    assert_eq!(occurrences[2].id().as_str(), "synthetic-all-day");
    assert!(matches!(
        occurrences[2].time(),
        CalendarOccurrenceTime::AllDay { start, end_exclusive }
            if *start == Date::from_calendar_date(2026, Month::November, 1).expect("valid test date")
                && *end_exclusive == Date::from_calendar_date(2026, Month::November, 2).expect("valid test date")
    ));
}

#[test]
fn maps_a_complete_empty_calendar_view() {
    let response = parse_calendar_view_snapshot(
        EMPTY_CALENDAR_VIEW_RESPONSE,
        EMPTY_CALENDAR_VIEW_RESPONSE.len(),
        &calendar_view(),
    )
    .expect("complete synthetic empty EWS view parses");
    let (occurrences, response_bytes) = response.into_parts();

    assert!(occurrences.is_empty());
    assert_eq!(response_bytes, EMPTY_CALENDAR_VIEW_RESPONSE.len());
}

#[test]
fn rejects_incomplete_ranges_and_invalid_calendar_responses() {
    let missing_response_code =
        replace_fixture_value("<m:ResponseCode>NoError</m:ResponseCode>", "");
    assert_eq!(
        parse_calendar_view_snapshot(
            &missing_response_code,
            missing_response_code.len(),
            &calendar_view(),
        ),
        Err(EwsError::InvalidResponse)
    );

    let incomplete = replace_fixture_value(
        "IncludesLastItemInRange=\"true\"",
        "IncludesLastItemInRange=\"false\"",
    );
    assert_eq!(
        parse_calendar_view_snapshot(&incomplete, incomplete.len(), &calendar_view()),
        Err(EwsError::ResourceLimitExceeded)
    );

    let unknown_zone =
        replace_fixture_value("Eastern Standard Time", "Unknown Synthetic Time Zone");
    assert_eq!(
        parse_calendar_view_snapshot(&unknown_zone, unknown_zone.len(), &calendar_view()),
        Err(EwsError::InvalidResponse)
    );

    let wrong_messages_namespace = replace_fixture_value(
        "xmlns:m=\"http://schemas.microsoft.com/exchange/services/2006/messages\"",
        "xmlns:m=\"urn:synthetic:wrong-messages\"",
    );
    assert_eq!(
        parse_calendar_view_snapshot(
            &wrong_messages_namespace,
            wrong_messages_namespace.len(),
            &calendar_view(),
        ),
        Err(EwsError::InvalidResponse)
    );

    let wrong_types_namespace = replace_fixture_value(
        "xmlns:t=\"http://schemas.microsoft.com/exchange/services/2006/types\"",
        "xmlns:t=\"urn:synthetic:wrong-types\"",
    );
    assert_eq!(
        parse_calendar_view_snapshot(
            &wrong_types_namespace,
            wrong_types_namespace.len(),
            &calendar_view(),
        ),
        Err(EwsError::InvalidResponse)
    );

    let missing_start = replace_fixture_value("<t:Start>2026-11-01T05:30:00Z</t:Start>", "");
    assert_eq!(
        parse_calendar_view_snapshot(&missing_start, missing_start.len(), &calendar_view()),
        Err(EwsError::InvalidResponse)
    );
}

#[test]
fn rejects_snapshots_above_the_calendar_view_limit() {
    let wire_items = (0..=MAX_CALENDAR_OCCURRENCES)
        .map(|_| {
            CalendarItemElement::CalendarItem(EwsCalendarItem {
                item_id: None,
                subject: None,
                location: None,
                start: None,
                end: None,
                is_all_day_event: None,
                calendar_item_type: None,
                start_time_zone_id: None,
                end_time_zone_id: None,
            })
        })
        .collect();

    assert_eq!(
        map_snapshot_items(wire_items, 0, &calendar_view()),
        Err(EwsError::ResourceLimitExceeded)
    );
}

#[test]
fn maps_access_denied_codes_and_preserves_other_calendar_response_errors() {
    for (response_code, expected_error) in [
        ("ErrorAccessDenied", EwsError::AccessDenied),
        ("ErrorImpersonateUserDenied", EwsError::AccessDenied),
        (
            "ErrorCalendarViewRangeTooBig",
            EwsError::ResourceLimitExceeded,
        ),
        ("ErrorInternalServerError", EwsError::InvalidResponse),
        ("ErrorSyntheticUnknown", EwsError::InvalidResponse),
    ] {
        let document = calendar_error_response(response_code);

        assert_eq!(
            parse_calendar_view_snapshot(&document, document.len(), &calendar_view()),
            Err(expected_error)
        );
    }
}

fn replace_fixture_value(original: &str, replacement: &str) -> Vec<u8> {
    String::from_utf8_lossy(CALENDAR_VIEW_RESPONSE)
        .replace(original, replacement)
        .into_bytes()
}

fn calendar_error_response(response_code: &str) -> Vec<u8> {
    let document = String::from_utf8_lossy(CALENDAR_VIEW_RESPONSE)
        .replace("ResponseClass=\"Success\"", "ResponseClass=\"Error\"")
        .replace(
            "<m:ResponseCode>NoError</m:ResponseCode>",
            &format!("<m:ResponseCode>{response_code}</m:ResponseCode>"),
        );
    document.into_bytes()
}

fn calendar_view() -> CalendarViewReference {
    let account_id = AccountId::try_from("synthetic-ews-account".to_owned())
        .expect("synthetic account ID is valid");
    let start = OffsetDateTime::parse(
        "2026-11-01T00:00:00Z",
        &time::format_description::well_known::Iso8601::DEFAULT,
    )
    .expect("synthetic view start is valid");
    let end = OffsetDateTime::parse(
        "2026-11-08T00:00:00Z",
        &time::format_description::well_known::Iso8601::DEFAULT,
    )
    .expect("synthetic view end is valid");
    let range = CalendarViewRange::new(
        start,
        end,
        Date::from_calendar_date(2026, Month::November, 1).expect("valid test date"),
        Date::from_calendar_date(2026, Month::November, 8).expect("valid test date"),
    )
    .expect("synthetic calendar view range is valid");
    CalendarViewReference::new(account_id, range)
}
