use einfach_mail_domain::{CalendarEventCreationId, EventId};
use ews::{
    BaseItemId, BaseShape, ItemShape, Message, PathToElement, PropertyType, RealItem,
    ResponseClass,
    get_item::{GetItem, GetItemResponseMessage},
    response::ResponseCode,
};
use quick_xml::{Reader, de::from_reader, events::Event};
use serde::{Deserialize, de::IgnoredAny};
use time::{OffsetDateTime, UtcOffset, format_description::well_known::Iso8601};

use crate::transport::{EwsSoapResponse, parse_soap_response};

use super::validate_calendar_response_namespaces;

pub(super) const OPERATION_MARKER_PROPERTY_SET_ID: &str = "4c2a2922-972e-54f9-bf4a-21e6b6d66d82";
pub(super) const OPERATION_MARKER_PROPERTY_NAME: &str = "EinfachMailCalendarOperationId";

const CALENDAR_ITEM_FIELD_URIS: [&str; 12] = [
    "item:Subject",
    "calendar:Start",
    "calendar:End",
    "calendar:CalendarItemType",
    "calendar:IsRecurring",
    "calendar:Recurrence",
    "calendar:IsAllDayEvent",
    "calendar:IsMeeting",
    "calendar:IsOnlineMeeting",
    "calendar:RequiredAttendees",
    "calendar:OptionalAttendees",
    "calendar:Resources",
];

#[derive(Debug, Eq, PartialEq)]
pub(super) struct CalendarItemSnapshot {
    pub(super) id: EventId,
    pub(super) change_key: String,
    pub(super) operation_marker: Option<CalendarEventCreationId>,
    pub(super) subject: String,
    pub(super) start: OffsetDateTime,
    pub(super) end: OffsetDateTime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CalendarItemSnapshotError {
    Rejected(ResponseCode),
    Ineligible,
    InvalidResponse,
}

pub(super) fn get_item_request(event_id: &EventId) -> GetItem {
    GetItem {
        item_shape: ItemShape {
            base_shape: BaseShape::IdOnly,
            include_mime_content: Some(false),
            additional_properties: Some(requested_calendar_item_properties()),
        },
        item_ids: vec![BaseItemId::ItemId {
            id: event_id.as_str().to_owned(),
            change_key: None,
        }],
    }
}

fn requested_calendar_item_properties() -> Vec<PathToElement> {
    let mut properties = CALENDAR_ITEM_FIELD_URIS
        .iter()
        .map(|field_uri| PathToElement::FieldURI {
            field_URI: (*field_uri).to_owned(),
        })
        .collect::<Vec<_>>();
    properties.push(PathToElement::ExtendedFieldURI {
        distinguished_property_set_id: None,
        property_set_id: Some(OPERATION_MARKER_PROPERTY_SET_ID.to_owned()),
        property_tag: None,
        property_name: Some(OPERATION_MARKER_PROPERTY_NAME.to_owned()),
        property_id: None,
        property_type: PropertyType::String,
    });
    properties
}

pub(super) fn parse_calendar_item_snapshot(
    document: &[u8],
    expected_id: &EventId,
) -> Result<CalendarItemSnapshot, CalendarItemSnapshotError> {
    validate_document(document)?;
    let calendar_item = parse_wire_calendar_item(document)?;
    let fields = snapshot_fields(calendar_item, expected_id)?;
    let message = parse_typed_calendar_item(document)?;
    validate_typed_item(&message, expected_id, &fields)?;
    let operation_marker = operation_marker(&message)?;

    Ok(CalendarItemSnapshot {
        id: expected_id.clone(),
        change_key: fields.change_key,
        operation_marker,
        subject: fields.subject,
        start: fields.start,
        end: fields.end,
    })
}

fn validate_document(document: &[u8]) -> Result<(), CalendarItemSnapshotError> {
    crate::transport::validate_soap_header(document)
        .map_err(|_| CalendarItemSnapshotError::InvalidResponse)?;
    validate_calendar_response_namespaces(document)
        .map_err(|_| CalendarItemSnapshotError::InvalidResponse)?;
    reject_recurrence_element(document)
}

fn reject_recurrence_element(document: &[u8]) -> Result<(), CalendarItemSnapshotError> {
    let mut reader = Reader::from_reader(document);
    loop {
        match reader
            .read_event()
            .map_err(|_| CalendarItemSnapshotError::InvalidResponse)?
        {
            Event::Start(element) | Event::Empty(element)
                if element.local_name().as_ref() == b"Recurrence" =>
            {
                return Err(CalendarItemSnapshotError::Ineligible);
            }
            Event::Eof => return Ok(()),
            _ => {}
        }
    }
}

fn parse_wire_calendar_item(
    document: &[u8],
) -> Result<CalendarItemShape, CalendarItemSnapshotError> {
    let envelope: CalendarItemSoapEnvelope =
        from_reader(document).map_err(|_| CalendarItemSnapshotError::Ineligible)?;
    successful_calendar_item(document, envelope)
}

fn snapshot_fields(
    calendar_item: CalendarItemShape,
    expected_id: &EventId,
) -> Result<CalendarItemSnapshotFields, CalendarItemSnapshotError> {
    let (change_key, subject) = snapshot_identity(&calendar_item, expected_id)?;
    validate_standalone_shape(&calendar_item)?;
    let (start, end) = snapshot_time_range(&calendar_item)?;

    Ok(CalendarItemSnapshotFields {
        change_key,
        subject,
        start,
        end,
    })
}

fn snapshot_identity(
    calendar_item: &CalendarItemShape,
    expected_id: &EventId,
) -> Result<(String, String), CalendarItemSnapshotError> {
    let item_id = calendar_item
        .item_id
        .as_ref()
        .ok_or(CalendarItemSnapshotError::Ineligible)?;
    let id = nonempty_text(item_id.id.as_deref())?;
    if id != expected_id.as_str() {
        return Err(CalendarItemSnapshotError::Ineligible);
    }
    let change_key = nonempty_text(item_id.change_key.as_deref())?.to_owned();
    let subject = nonempty_text(calendar_item.subject.as_deref())?.to_owned();
    Ok((change_key, subject))
}

fn nonempty_text(text: Option<&str>) -> Result<&str, CalendarItemSnapshotError> {
    text.filter(|text| !text.trim().is_empty() && !text.chars().any(char::is_control))
        .ok_or(CalendarItemSnapshotError::Ineligible)
}

fn snapshot_time_range(
    calendar_item: &CalendarItemShape,
) -> Result<(OffsetDateTime, OffsetDateTime), CalendarItemSnapshotError> {
    let start = parse_utc_datetime(calendar_item.start.as_deref())?;
    let end = parse_utc_datetime(calendar_item.end.as_deref())?;
    if end <= start {
        return Err(CalendarItemSnapshotError::Ineligible);
    }
    Ok((start, end))
}

struct CalendarItemSnapshotFields {
    change_key: String,
    subject: String,
    start: OffsetDateTime,
    end: OffsetDateTime,
}

fn successful_calendar_item(
    document: &[u8],
    envelope: CalendarItemSoapEnvelope,
) -> Result<CalendarItemShape, CalendarItemSnapshotError> {
    let message = only_wire_response_message(envelope)?;
    validate_wire_response_class(document, &message)?;
    only_wire_calendar_item(message)
}

fn only_wire_response_message(
    envelope: CalendarItemSoapEnvelope,
) -> Result<CalendarItemSoapResponseMessage, CalendarItemSnapshotError> {
    let mut messages = envelope.body.get_item_response.response_messages.inner;
    if messages.len() != 1 {
        return Err(CalendarItemSnapshotError::InvalidResponse);
    }
    messages
        .pop()
        .ok_or(CalendarItemSnapshotError::InvalidResponse)
}

fn validate_wire_response_class(
    document: &[u8],
    message: &CalendarItemSoapResponseMessage,
) -> Result<(), CalendarItemSnapshotError> {
    match (message.response_class.as_deref(), message.response_code) {
        (Some("Success"), Some(ResponseCode::NoError)) => Ok(()),
        (Some("Error"), Some(code)) if code != ResponseCode::NoError => {
            validate_error_response(document, code)?;
            Err(CalendarItemSnapshotError::Rejected(code))
        }
        _ => Err(CalendarItemSnapshotError::InvalidResponse),
    }
}

fn only_wire_calendar_item(
    message: CalendarItemSoapResponseMessage,
) -> Result<CalendarItemShape, CalendarItemSnapshotError> {
    let mut items = message
        .items
        .ok_or(CalendarItemSnapshotError::InvalidResponse)?
        .inner;
    if items.len() != 1 {
        return Err(CalendarItemSnapshotError::InvalidResponse);
    }
    match items
        .pop()
        .ok_or(CalendarItemSnapshotError::InvalidResponse)?
    {
        CalendarItemElement::CalendarItem(item) => Ok(*item),
        _ => Err(CalendarItemSnapshotError::Ineligible),
    }
}

fn validate_error_response(
    document: &[u8],
    expected_code: ResponseCode,
) -> Result<(), CalendarItemSnapshotError> {
    let response = parse_soap_response::<GetItem>(document)
        .map_err(|_| CalendarItemSnapshotError::InvalidResponse)?;
    let [ResponseClass::Error(error)] = response.messages.as_slice() else {
        return Err(CalendarItemSnapshotError::InvalidResponse);
    };
    if error.response_code != expected_code {
        return Err(CalendarItemSnapshotError::InvalidResponse);
    }
    Ok(())
}

fn parse_typed_calendar_item(document: &[u8]) -> Result<Message, CalendarItemSnapshotError> {
    let response = parse_soap_response::<GetItem>(document)
        .map_err(|_| CalendarItemSnapshotError::Ineligible)?;
    successful_calendar_item_message(response)
}

fn successful_calendar_item_message(
    response: EwsSoapResponse<GetItemResponseMessage>,
) -> Result<Message, CalendarItemSnapshotError> {
    let mut messages = response.messages;
    if messages.len() != 1 {
        return Err(CalendarItemSnapshotError::InvalidResponse);
    }
    let Some(ResponseClass::Success(response)) = messages.pop() else {
        return Err(CalendarItemSnapshotError::InvalidResponse);
    };
    let mut items = response.items.inner;
    if items.len() != 1 {
        return Err(CalendarItemSnapshotError::InvalidResponse);
    }
    match items.pop() {
        Some(RealItem::CalendarItem(message)) => Ok(message),
        Some(_) => Err(CalendarItemSnapshotError::Ineligible),
        None => Err(CalendarItemSnapshotError::InvalidResponse),
    }
}

fn validate_typed_item(
    message: &Message,
    expected_id: &EventId,
    fields: &CalendarItemSnapshotFields,
) -> Result<(), CalendarItemSnapshotError> {
    let item_id = message
        .item_id
        .as_ref()
        .ok_or(CalendarItemSnapshotError::Ineligible)?;
    if item_id.id != expected_id.as_str()
        || item_id.change_key.as_deref() != Some(fields.change_key.as_str())
        || message.subject.as_deref() != Some(fields.subject.as_str())
    {
        return Err(CalendarItemSnapshotError::Ineligible);
    }
    Ok(())
}

fn operation_marker(
    message: &Message,
) -> Result<Option<CalendarEventCreationId>, CalendarItemSnapshotError> {
    let Some(property) = message
        .extended_property
        .as_deref()
        .map(operation_marker_property)
        .transpose()?
        .flatten()
    else {
        return Ok(None);
    };
    let uri = &property.extended_field_URI;
    if uri.distinguished_property_set_id.is_some()
        || uri.property_tag.is_some()
        || uri.property_id.is_some()
        || uri.property_type != Some(PropertyType::String)
    {
        return Err(CalendarItemSnapshotError::Ineligible);
    }
    CalendarEventCreationId::parse(&property.value)
        .map(Some)
        .map_err(|_| CalendarItemSnapshotError::Ineligible)
}

fn operation_marker_property(
    properties: &[ews::ExtendedProperty],
) -> Result<Option<&ews::ExtendedProperty>, CalendarItemSnapshotError> {
    let mut matches = properties.iter().filter(|property| {
        let uri = &property.extended_field_URI;
        uri.property_set_id
            .as_deref()
            .is_some_and(|id| id.eq_ignore_ascii_case(OPERATION_MARKER_PROPERTY_SET_ID))
            && uri.property_name.as_deref() == Some(OPERATION_MARKER_PROPERTY_NAME)
    });
    let Some(property) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Err(CalendarItemSnapshotError::Ineligible);
    }
    Ok(Some(property))
}

fn validate_standalone_shape(
    calendar_item: &CalendarItemShape,
) -> Result<(), CalendarItemSnapshotError> {
    if calendar_item.calendar_item_type.as_deref() != Some("Single")
        || calendar_item.is_recurring != Some(false)
        || calendar_item.is_all_day_event != Some(false)
        || calendar_item.is_meeting != Some(false)
        || calendar_item.is_online_meeting != Some(false)
        || contains_attendees(&calendar_item.required_attendees)
        || contains_attendees(&calendar_item.optional_attendees)
        || contains_attendees(&calendar_item.resources)
    {
        return Err(CalendarItemSnapshotError::Ineligible);
    }
    Ok(())
}

fn contains_attendees(collection: &Option<Attendees>) -> bool {
    collection
        .as_ref()
        .is_some_and(|attendees| !attendees.entries.is_empty())
}

fn parse_utc_datetime(value: Option<&str>) -> Result<OffsetDateTime, CalendarItemSnapshotError> {
    let datetime_text = value.ok_or(CalendarItemSnapshotError::Ineligible)?;
    let datetime = OffsetDateTime::parse(datetime_text, &Iso8601::DEFAULT)
        .map_err(|_| CalendarItemSnapshotError::Ineligible)?;
    if datetime.offset() != UtcOffset::UTC {
        return Err(CalendarItemSnapshotError::Ineligible);
    }
    Ok(datetime)
}

#[derive(Deserialize)]
struct CalendarItemSoapEnvelope {
    #[serde(rename = "Body")]
    body: CalendarItemSoapBody,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CalendarItemSoapBody {
    get_item_response: CalendarItemSoapResponse,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CalendarItemSoapResponse {
    response_messages: CalendarItemSoapResponseMessages,
}

#[derive(Deserialize)]
struct CalendarItemSoapResponseMessages {
    #[serde(rename = "$value", default)]
    inner: Vec<CalendarItemSoapResponseMessage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CalendarItemSoapResponseMessage {
    #[serde(rename = "@ResponseClass")]
    response_class: Option<String>,
    response_code: Option<ResponseCode>,
    items: Option<CalendarItems>,
}

#[derive(Deserialize)]
struct CalendarItems {
    #[serde(rename = "$value", default)]
    inner: Vec<CalendarItemElement>,
}

#[derive(Deserialize)]
enum CalendarItemElement {
    CalendarItem(Box<CalendarItemShape>),
    Item(IgnoredAny),
    Message(IgnoredAny),
    Contact(IgnoredAny),
    DistributionList(IgnoredAny),
    MeetingMessage(IgnoredAny),
    MeetingRequest(IgnoredAny),
    MeetingResponse(IgnoredAny),
    MeetingCancellation(IgnoredAny),
    Task(IgnoredAny),
    PostItem(IgnoredAny),
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CalendarItemShape {
    item_id: Option<CalendarItemId>,
    subject: Option<String>,
    calendar_item_type: Option<String>,
    is_recurring: Option<bool>,
    is_all_day_event: Option<bool>,
    is_meeting: Option<bool>,
    is_online_meeting: Option<bool>,
    start: Option<String>,
    end: Option<String>,
    required_attendees: Option<Attendees>,
    optional_attendees: Option<Attendees>,
    resources: Option<Attendees>,
}

#[derive(Deserialize)]
struct CalendarItemId {
    #[serde(rename = "@Id")]
    id: Option<String>,
    #[serde(rename = "@ChangeKey")]
    change_key: Option<String>,
}

#[derive(Deserialize)]
struct Attendees {
    #[serde(rename = "$value", default)]
    entries: Vec<IgnoredAny>,
}

#[cfg(test)]
mod tests;
