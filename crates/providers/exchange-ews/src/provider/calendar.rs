use std::str::FromStr;

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

use std::{future::Future, sync::Arc};

use einfach_mail_application::{
    CalendarDeltaRequest, CalendarDeltaResponse, CalendarOperationError, CalendarProvider,
    CalendarSyncStrategy, CalendarViewSnapshotResponse, ExchangeEwsAccountSettingsStore,
};
use einfach_mail_provider_runtime as worker;
use tokio_util::sync::CancellationToken;

use super::{ExchangeEwsProvider, session::session_for_calendar};

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

impl<S> CalendarProvider for ExchangeEwsProvider<S>
where
    S: ExchangeEwsAccountSettingsStore + 'static,
{
    fn calendar_sync_strategy(
        &self,
    ) -> impl Future<Output = Result<CalendarSyncStrategy, CalendarOperationError>> + Send + '_
    {
        std::future::ready(Ok(CalendarSyncStrategy::Snapshot))
    }

    fn calendar_delta_page(
        &self,
        _view: &einfach_mail_domain::CalendarViewReference,
        _request: CalendarDeltaRequest,
    ) -> impl Future<Output = Result<CalendarDeltaResponse, CalendarOperationError>> + Send + '_
    {
        std::future::ready(Err(CalendarOperationError::UnsupportedOperation))
    }

    fn calendar_view_snapshot(
        &self,
        view: &einfach_mail_domain::CalendarViewReference,
    ) -> impl Future<Output = Result<CalendarViewSnapshotResponse, CalendarOperationError>> + Send + '_
    {
        let view = view.clone();
        let session_store = Arc::clone(&self.session);
        async move {
            let session = session_for_calendar(&session_store, view.account_id()).await?;
            worker::run(move |_cancellation: CancellationToken| async move {
                let request =
                    build_calendar_view_request(&view).map_err(CalendarOperationError::from)?;
                let (document, response_bytes) = session
                    .transport
                    .execute_xml_with_size(request)
                    .await
                    .map_err(CalendarOperationError::from)?;
                parse_calendar_view_snapshot(&document, response_bytes, &view)
                    .map_err(CalendarOperationError::from)
            })
            .await
        }
    }

    fn create_event(
        &self,
        _creation: &einfach_mail_domain::CalendarEventCreation,
    ) -> impl Future<
        Output = Result<einfach_mail_domain::CalendarOccurrence, CalendarOperationError>,
    > + Send
    + '_ {
        std::future::ready(Err(CalendarOperationError::UnsupportedOperation))
    }

    fn editable_event(
        &self,
        _event: &einfach_mail_domain::CalendarOccurrence,
    ) -> impl Future<
        Output = Result<einfach_mail_domain::EditableCalendarEvent, CalendarOperationError>,
    > + Send
    + '_ {
        std::future::ready(Err(CalendarOperationError::UnsupportedOperation))
    }

    fn update_event(
        &self,
        _update: &einfach_mail_domain::CalendarEventUpdate,
    ) -> impl Future<
        Output = Result<einfach_mail_domain::CalendarOccurrence, CalendarOperationError>,
    > + Send
    + '_ {
        std::future::ready(Err(CalendarOperationError::UnsupportedOperation))
    }

    fn delete_event(
        &self,
        _event: &einfach_mail_domain::EditableCalendarEvent,
    ) -> impl Future<Output = Result<(), CalendarOperationError>> + Send + '_ {
        std::future::ready(Err(CalendarOperationError::UnsupportedOperation))
    }
}

#[cfg(test)]
mod tests;
