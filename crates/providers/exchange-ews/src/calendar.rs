use std::str::FromStr;

use einfach_mail_application::CalendarViewSnapshotResponse;
use einfach_mail_domain::{
    CalendarOccurrence, CalendarOccurrencePatch, CalendarOccurrenceTimePatch,
    CalendarViewReference, EventId,
};
use ews::{
    BaseFolderId, BaseShape, ItemShape, PathToElement, View,
    find_item::{FindItem, Traversal},
};
use jiff::{Timestamp, tz::TimeZone};
use quick_xml::de::from_reader;
use serde::Deserialize;
use time::{Date, Month, OffsetDateTime, UtcOffset, format_description::well_known::Iso8601};
use windows_timezones::WindowsTimezone;

use crate::{
    error::EwsError,
    transport::{SOAP_NAMESPACE, validate_soap_header},
};

const MAX_CALENDAR_OCCURRENCES: usize = 512;
const EWS_MESSAGES_NAMESPACE: &[u8] =
    b"http://schemas.microsoft.com/exchange/services/2006/messages";
const EWS_TYPES_NAMESPACE: &[u8] = b"http://schemas.microsoft.com/exchange/services/2006/types";
const CALENDAR_FIELDS: [&str; 8] = [
    "item:Subject",
    "calendar:Location",
    "calendar:Start",
    "calendar:End",
    "calendar:IsAllDayEvent",
    "calendar:CalendarItemType",
    "calendar:StartTimeZoneId",
    "calendar:EndTimeZoneId",
];

pub(crate) fn build_calendar_view_request(
    view: &CalendarViewReference,
) -> Result<FindItem, EwsError> {
    let start_date = format_ews_datetime(view.range().start())?;
    let end_date = format_ews_datetime(view.range().end())?;
    Ok(FindItem {
        traversal: Traversal::Shallow,
        item_shape: calendar_item_shape(),
        view: Some(View::CalendarView {
            max_entries_returned: Some(MAX_CALENDAR_OCCURRENCES),
            start_date,
            end_date,
        }),
        parent_folder_ids: vec![BaseFolderId::new_distinguished("calendar")],
    })
}

fn calendar_item_shape() -> ItemShape {
    ItemShape {
        base_shape: BaseShape::IdOnly,
        include_mime_content: Some(false),
        additional_properties: Some(
            CALENDAR_FIELDS
                .iter()
                .map(|field_uri| PathToElement::FieldURI {
                    field_URI: (*field_uri).to_owned(),
                })
                .collect(),
        ),
    }
}

pub(crate) fn parse_calendar_view_snapshot(
    document: &[u8],
    response_bytes: usize,
    view: &CalendarViewReference,
) -> Result<CalendarViewSnapshotResponse, EwsError> {
    validate_soap_header(document)?;
    validate_calendar_response_namespaces(document)?;
    let envelope: CalendarSoapEnvelope =
        from_reader(document).map_err(|_| EwsError::InvalidResponse)?;
    let response = only_response_message(envelope.body.find_item_response.response_messages.inner)?;
    let root = complete_root_folder(response)?;
    let wire_items = root.items.ok_or(EwsError::InvalidResponse)?.inner;
    map_snapshot_items(wire_items, response_bytes, view)
}

fn validate_calendar_response_namespaces(document: &[u8]) -> Result<(), EwsError> {
    use quick_xml::{NsReader, events::Event, name::ResolveResult};

    let mut reader = NsReader::from_reader(document);
    loop {
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|_| EwsError::InvalidResponse)?;
        let element = match event {
            Event::Start(element) | Event::Empty(element) => element,
            Event::Eof => return Ok(()),
            _ => continue,
        };
        let Some(expected_namespace) = expected_calendar_namespace(element.local_name().as_ref())
        else {
            continue;
        };
        if !matches!(namespace, ResolveResult::Bound(actual) if actual.as_ref() == expected_namespace)
        {
            return Err(EwsError::InvalidResponse);
        }
    }
}

fn expected_calendar_namespace(local_name: &[u8]) -> Option<&'static [u8]> {
    match local_name {
        b"Envelope" | b"Header" | b"Body" => Some(SOAP_NAMESPACE),
        b"FindItemResponse"
        | b"ResponseMessages"
        | b"FindItemResponseMessage"
        | b"RootFolder"
        | b"ResponseCode" => Some(EWS_MESSAGES_NAMESPACE),
        b"ServerVersionInfo" | b"Items" | b"CalendarItem" | b"ItemId" | b"Subject"
        | b"Location" | b"Start" | b"End" | b"IsAllDayEvent" | b"CalendarItemType"
        | b"StartTimeZoneId" | b"EndTimeZoneId" => Some(EWS_TYPES_NAMESPACE),
        _ => None,
    }
}

fn format_ews_datetime(datetime: OffsetDateTime) -> Result<String, EwsError> {
    datetime
        .format(&Iso8601::DEFAULT)
        .map_err(|_| EwsError::InvalidResponse)
}

fn only_response_message(
    mut responses: Vec<CalendarResponseMessage>,
) -> Result<CalendarResponseMessage, EwsError> {
    if responses.len() != 1 {
        return Err(EwsError::InvalidResponse);
    }
    let response = responses.pop().ok_or(EwsError::InvalidResponse)?;
    if response.response_class.as_deref() != Some("Success")
        || response.response_code.as_deref() != Some("NoError")
    {
        return Err(map_response_error(response.response_code.as_deref()));
    }
    Ok(response)
}

fn map_response_error(response_code: Option<&str>) -> EwsError {
    match response_code {
        Some("ErrorAccessDenied" | "ErrorImpersonateUserDenied") => EwsError::AccessDenied,
        Some("ErrorCalendarViewRangeTooBig") => EwsError::ResourceLimitExceeded,
        _ => EwsError::InvalidResponse,
    }
}

fn complete_root_folder(response: CalendarResponseMessage) -> Result<CalendarRootFolder, EwsError> {
    let root = response.root_folder.ok_or(EwsError::InvalidResponse)?;
    if root.includes_last_item_in_range != Some(true) {
        return Err(EwsError::ResourceLimitExceeded);
    }
    Ok(root)
}

fn map_snapshot_items(
    wire_items: Vec<CalendarItemElement>,
    response_bytes: usize,
    view: &CalendarViewReference,
) -> Result<CalendarViewSnapshotResponse, EwsError> {
    if wire_items.len() > MAX_CALENDAR_OCCURRENCES {
        return Err(EwsError::ResourceLimitExceeded);
    }
    let occurrences = map_calendar_items(wire_items, view)?;
    if has_duplicate_event_ids(&occurrences) {
        return Err(EwsError::InvalidResponse);
    }
    Ok(CalendarViewSnapshotResponse::new(
        occurrences,
        response_bytes,
    ))
}

fn map_calendar_items(
    wire_items: Vec<CalendarItemElement>,
    view: &CalendarViewReference,
) -> Result<Vec<CalendarOccurrence>, EwsError> {
    wire_items
        .into_iter()
        .map(|element| match element {
            CalendarItemElement::CalendarItem(calendar_item) => {
                map_calendar_item(calendar_item, view)
            }
        })
        .collect()
}

fn has_duplicate_event_ids(occurrences: &[CalendarOccurrence]) -> bool {
    let mut seen_ids = std::collections::HashSet::with_capacity(occurrences.len());
    occurrences
        .iter()
        .any(|occurrence| !seen_ids.insert(occurrence.id().as_str()))
}

fn map_calendar_item(
    calendar_item: EwsCalendarItem,
    view: &CalendarViewReference,
) -> Result<CalendarOccurrence, EwsError> {
    validate_calendar_item_type(&calendar_item)?;
    let occurrence_time = map_occurrence_time(&calendar_item)?;
    let event_id = EventId::try_from(calendar_item.item_id.ok_or(EwsError::InvalidResponse)?.id)
        .map_err(|_| EwsError::InvalidResponse)?;
    let mut patch = CalendarOccurrencePatch::new();
    patch.set_subject(calendar_item.subject);
    patch.set_location(calendar_item.location);
    patch.set_original_start_time_zone(calendar_item.start_time_zone_id);
    patch.set_original_end_time_zone(calendar_item.end_time_zone_id);
    patch.set_time(occurrence_time);
    patch
        .into_new(view.clone(), event_id)
        .map_err(|_| EwsError::InvalidResponse)
}

fn validate_calendar_item_type(calendar_item: &EwsCalendarItem) -> Result<(), EwsError> {
    match calendar_item.calendar_item_type.as_deref() {
        Some("Single" | "Occurrence" | "Exception") => Ok(()),
        _ => Err(EwsError::InvalidResponse),
    }
}

fn map_occurrence_time(
    calendar_item: &EwsCalendarItem,
) -> Result<CalendarOccurrenceTimePatch, EwsError> {
    let (start, end) = map_calendar_item_times(calendar_item)?;
    let mut time = CalendarOccurrenceTimePatch::new();
    time.set_start(start);
    time.set_end(end);
    time.set_is_all_day(
        calendar_item
            .is_all_day_event
            .ok_or(EwsError::InvalidResponse)?,
    );
    Ok(time)
}

fn map_calendar_item_times(
    calendar_item: &EwsCalendarItem,
) -> Result<(OffsetDateTime, OffsetDateTime), EwsError> {
    let start = parse_ews_datetime(
        calendar_item
            .start
            .as_deref()
            .ok_or(EwsError::InvalidResponse)?,
    )?;
    let end = parse_ews_datetime(
        calendar_item
            .end
            .as_deref()
            .ok_or(EwsError::InvalidResponse)?,
    )?;
    if calendar_item
        .is_all_day_event
        .ok_or(EwsError::InvalidResponse)?
    {
        map_all_day_bounds(calendar_item, start, end)
    } else {
        Ok((
            start.to_offset(UtcOffset::UTC),
            end.to_offset(UtcOffset::UTC),
        ))
    }
}

fn parse_ews_datetime(date_time_value: &str) -> Result<OffsetDateTime, EwsError> {
    if !has_explicit_offset(date_time_value) {
        return Err(EwsError::InvalidResponse);
    }
    OffsetDateTime::parse(date_time_value, &Iso8601::DEFAULT).map_err(|_| EwsError::InvalidResponse)
}

fn has_explicit_offset(date_time_value: &str) -> bool {
    if date_time_value.ends_with('Z') {
        return true;
    }
    let Some(time_separator) = date_time_value.find('T') else {
        return false;
    };
    date_time_value
        .rfind(['+', '-'])
        .is_some_and(|offset_separator| offset_separator > time_separator)
}

fn map_all_day_bounds(
    calendar_item: &EwsCalendarItem,
    start: OffsetDateTime,
    end: OffsetDateTime,
) -> Result<(OffsetDateTime, OffsetDateTime), EwsError> {
    let start_zone = calendar_item
        .start_time_zone_id
        .as_deref()
        .ok_or(EwsError::InvalidResponse)?;
    let end_zone = calendar_item
        .end_time_zone_id
        .as_deref()
        .ok_or(EwsError::InvalidResponse)?;
    let start_date = local_date_for_instant(start, start_zone)?;
    let end_date = local_date_for_instant(end, end_zone)?;
    Ok((
        start_date.midnight().assume_utc(),
        end_date.midnight().assume_utc(),
    ))
}

fn local_date_for_instant(
    instant: OffsetDateTime,
    windows_zone_id: &str,
) -> Result<Date, EwsError> {
    let windows_zone =
        WindowsTimezone::from_str(windows_zone_id).map_err(|_| EwsError::InvalidResponse)?;
    let time_zone = TimeZone::get(windows_zone.tzdb_id()).map_err(|_| EwsError::InvalidResponse)?;
    let nanosecond = i32::try_from(instant.nanosecond()).map_err(|_| EwsError::InvalidResponse)?;
    let timestamp = Timestamp::new(instant.unix_timestamp(), nanosecond)
        .map_err(|_| EwsError::InvalidResponse)?;
    let local_date = timestamp.to_zoned(time_zone).date();
    let year = i32::from(local_date.year());
    let month = u8::try_from(local_date.month()).map_err(|_| EwsError::InvalidResponse)?;
    let month = Month::try_from(month).map_err(|_| EwsError::InvalidResponse)?;
    let day = u8::try_from(local_date.day()).map_err(|_| EwsError::InvalidResponse)?;
    Date::from_calendar_date(year, month, day).map_err(|_| EwsError::InvalidResponse)
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CalendarSoapEnvelope {
    body: CalendarSoapBody,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CalendarSoapBody {
    find_item_response: CalendarFindItemResponse,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CalendarFindItemResponse {
    response_messages: CalendarResponseMessages,
}

#[derive(Deserialize)]
struct CalendarResponseMessages {
    #[serde(rename = "$value", default)]
    inner: Vec<CalendarResponseMessage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CalendarResponseMessage {
    #[serde(rename = "@ResponseClass")]
    response_class: Option<String>,
    root_folder: Option<CalendarRootFolder>,
    response_code: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CalendarRootFolder {
    #[serde(rename = "@IncludesLastItemInRange")]
    includes_last_item_in_range: Option<bool>,
    items: Option<CalendarItems>,
}

#[derive(Deserialize)]
struct CalendarItems {
    #[serde(rename = "$value", default)]
    inner: Vec<CalendarItemElement>,
}

#[derive(Deserialize)]
enum CalendarItemElement {
    CalendarItem(EwsCalendarItem),
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct EwsCalendarItem {
    item_id: Option<EwsItemId>,
    subject: Option<String>,
    location: Option<String>,
    start: Option<String>,
    end: Option<String>,
    is_all_day_event: Option<bool>,
    calendar_item_type: Option<String>,
    start_time_zone_id: Option<String>,
    end_time_zone_id: Option<String>,
}

#[derive(Deserialize)]
struct EwsItemId {
    #[serde(rename = "@Id")]
    id: String,
}

#[cfg(test)]
mod tests {
    use einfach_mail_domain::{
        AccountId, CalendarOccurrenceTime, CalendarViewRange, CalendarViewReference,
    };
    use time::{Date, Month, OffsetDateTime};

    use crate::transport::serialize_soap_request;

    use super::{
        CalendarItemElement, EwsCalendarItem, EwsError, MAX_CALENDAR_OCCURRENCES,
        build_calendar_view_request, map_snapshot_items, parse_calendar_view_snapshot,
    };

    const CALENDAR_VIEW_RESPONSE: &[u8] = include_bytes!("fixtures/calendar-view-response.xml");
    const EMPTY_CALENDAR_VIEW_RESPONSE: &[u8] =
        include_bytes!("fixtures/empty-calendar-view-response.xml");

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
        assert!(
            xml.contains("<t:DistinguishedFolderId Id=\"calendar\"></t:DistinguishedFolderId>")
        );
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

    fn replace_fixture_value(original: &str, replacement: &str) -> Vec<u8> {
        String::from_utf8_lossy(CALENDAR_VIEW_RESPONSE)
            .replace(original, replacement)
            .into_bytes()
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
}
