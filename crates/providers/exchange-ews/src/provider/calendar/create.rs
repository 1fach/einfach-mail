use einfach_mail_application::CalendarOperationError;
use einfach_mail_domain::{
    CalendarEventCreation, CalendarOccurrence, CalendarOccurrencePatch, CalendarOccurrenceTime,
    CalendarOccurrenceTimePatch, EventId,
};
use ews::{
    BaseFolderId, ExtendedFieldURI, Message, PropertyType, RealItem, create_item::CreateItem,
    response::ResponseCode,
};
use quick_xml::{
    NsReader, Writer,
    events::{BytesEnd, BytesStart, BytesText, Event},
    name::ResolveResult,
};
use serde::Deserialize;
use time::{UtcOffset, format_description::well_known::Iso8601};

use crate::{
    error::EwsError,
    transport::{EwsRequestSequence, EwsWriteError, serialize_soap_request},
};

use super::{
    EWS_MESSAGES_NAMESPACE, EWS_TYPES_NAMESPACE,
    item::{
        CalendarItemSnapshot, CalendarItemSnapshotError, OPERATION_MARKER_PROPERTY_NAME,
        OPERATION_MARKER_PROPERTY_SET_ID, get_item_request, parse_calendar_item_snapshot,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CreateItemResponseError {
    Rejected,
    Unknown,
}

pub(super) async fn create_event(
    sequence: &EwsRequestSequence<'_>,
    creation: &CalendarEventCreation,
) -> Result<CalendarOccurrence, CalendarOperationError> {
    let request = create_item_request(creation).map_err(CalendarOperationError::from)?;
    let response = match sequence.execute_xml_write(&request).await {
        Ok(response) => response,
        Err(EwsWriteError::NotDispatched(error)) => return Err(error.into()),
        Err(EwsWriteError::OutcomeUnknown) => {
            return Err(CalendarOperationError::CreationOutcomeUnknown);
        }
    };
    let event_id = parse_create_item_response(&response).map_err(map_create_response_error)?;
    load_confirmed_event(sequence, creation, &event_id).await
}

fn map_create_response_error(error: CreateItemResponseError) -> CalendarOperationError {
    match error {
        CreateItemResponseError::Rejected => CalendarOperationError::EventCreationRejected,
        CreateItemResponseError::Unknown => CalendarOperationError::CreationOutcomeUnknown,
    }
}

pub(super) async fn load_confirmed_event(
    sequence: &EwsRequestSequence<'_>,
    creation: &CalendarEventCreation,
    event_id: &EventId,
) -> Result<CalendarOccurrence, CalendarOperationError> {
    let (document, _) = sequence
        .execute_xml_with_size(get_item_request(event_id))
        .await
        .map_err(|_| CalendarOperationError::CreationOutcomeUnknown)?;
    let snapshot =
        parse_calendar_item_snapshot(&document, event_id).map_err(map_snapshot_error_to_unknown)?;
    confirm_created_snapshot(creation, snapshot)
}

fn map_snapshot_error_to_unknown(_: CalendarItemSnapshotError) -> CalendarOperationError {
    CalendarOperationError::CreationOutcomeUnknown
}

fn confirm_created_snapshot(
    creation: &CalendarEventCreation,
    snapshot: CalendarItemSnapshot,
) -> Result<CalendarOccurrence, CalendarOperationError> {
    if !snapshot_matches_creation(creation, &snapshot) {
        return Err(CalendarOperationError::CreationOutcomeUnknown);
    }
    occurrence_from_snapshot(creation, snapshot)
}

fn snapshot_matches_creation(
    creation: &CalendarEventCreation,
    snapshot: &CalendarItemSnapshot,
) -> bool {
    let CalendarOccurrenceTime::Timed { start, end } = creation.draft().time() else {
        return false;
    };
    snapshot.operation_marker == Some(creation.id())
        && snapshot.subject == creation.draft().subject()
        && snapshot.start == *start
        && snapshot.end == *end
}

fn occurrence_from_snapshot(
    creation: &CalendarEventCreation,
    snapshot: CalendarItemSnapshot,
) -> Result<CalendarOccurrence, CalendarOperationError> {
    let mut time = CalendarOccurrenceTimePatch::new();
    time.set_start(snapshot.start);
    time.set_end(snapshot.end);
    time.set_is_all_day(false);
    let mut patch = CalendarOccurrencePatch::new();
    patch.set_subject(Some(snapshot.subject));
    patch.set_time(time);
    patch
        .into_new(creation.view().clone(), snapshot.id)
        .map_err(|_| CalendarOperationError::CreationOutcomeUnknown)
}

pub(super) fn create_item_request(creation: &CalendarEventCreation) -> Result<Vec<u8>, EwsError> {
    let (start, end) = utc_creation_bounds(creation)?;
    let document = serialize_soap_request(create_calendar_item_operation(creation))?;
    add_calendar_fields(&document, start, end)
}

fn utc_creation_bounds(
    creation: &CalendarEventCreation,
) -> Result<(time::OffsetDateTime, time::OffsetDateTime), EwsError> {
    let CalendarOccurrenceTime::Timed { start, end } = creation.draft().time() else {
        return Err(EwsError::InvalidResponse);
    };
    if start.offset() != UtcOffset::UTC || end.offset() != UtcOffset::UTC {
        return Err(EwsError::InvalidResponse);
    }
    Ok((*start, *end))
}

fn create_calendar_item_operation(creation: &CalendarEventCreation) -> CreateItem {
    let item = Message {
        subject: Some(creation.draft().subject().to_owned()),
        extended_property: Some(vec![operation_marker_property(
            creation.id().as_uuid().to_string(),
        )]),
        ..Message::default()
    };
    CreateItem {
        message_disposition: None,
        saved_item_folder_id: Some(BaseFolderId::new_distinguished("calendar")),
        items: vec![RealItem::CalendarItem(item)],
    }
}

fn operation_marker_property(value: String) -> ews::ExtendedProperty {
    ews::ExtendedProperty {
        extended_field_URI: ExtendedFieldURI {
            distinguished_property_set_id: None,
            property_set_id: Some(OPERATION_MARKER_PROPERTY_SET_ID.to_owned()),
            property_tag: None,
            property_name: Some(OPERATION_MARKER_PROPERTY_NAME.to_owned()),
            property_id: None,
            property_type: Some(PropertyType::String),
        },
        value,
    }
}

fn add_calendar_fields(
    document: &[u8],
    start: time::OffsetDateTime,
    end: time::OffsetDateTime,
) -> Result<Vec<u8>, EwsError> {
    let start = format_ews_datetime(start)?;
    let end = format_ews_datetime(end)?;
    let mut reader = NsReader::from_reader(document);
    let mut transform = CreateItemXmlTransform::new(document.len(), start, end);
    loop {
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|_| EwsError::InvalidResponse)?;
        if matches!(&event, Event::Eof) {
            break;
        }
        transform.write_event(&namespace, event)?;
    }
    transform.finish()
}

fn format_ews_datetime(value: time::OffsetDateTime) -> Result<String, EwsError> {
    value
        .format(&Iso8601::DEFAULT)
        .map_err(|_| EwsError::InvalidResponse)
}

struct CreateItemXmlTransform {
    writer: Writer<Vec<u8>>,
    parents: Vec<Vec<u8>>,
    start: String,
    end: String,
    create_item_seen: bool,
    calendar_item_seen: bool,
    subject_seen: bool,
    calendar_fields_written: bool,
}

impl CreateItemXmlTransform {
    fn new(document_len: usize, start: String, end: String) -> Self {
        Self {
            writer: Writer::new(Vec::with_capacity(document_len + 128)),
            parents: Vec::new(),
            start,
            end,
            create_item_seen: false,
            calendar_item_seen: false,
            subject_seen: false,
            calendar_fields_written: false,
        }
    }

    fn write_event(
        &mut self,
        namespace: &ResolveResult<'_>,
        event: Event<'_>,
    ) -> Result<(), EwsError> {
        match event {
            Event::Start(element) => self.write_start_event(namespace, element),
            Event::End(element) => self.write_end_event(element),
            event => self
                .writer
                .write_event(event.into_owned())
                .map_err(|_| EwsError::InvalidResponse),
        }
    }

    fn write_start_event(
        &mut self,
        namespace: &ResolveResult<'_>,
        element: quick_xml::events::BytesStart<'_>,
    ) -> Result<(), EwsError> {
        if is_element(namespace, &element, EWS_MESSAGES_NAMESPACE, b"CreateItem") {
            return self.write_create_item_start(element);
        }
        if is_element(namespace, &element, EWS_TYPES_NAMESPACE, b"CalendarItem") {
            return self.write_calendar_item_start(element);
        }
        self.write_nested_start(element)
    }

    fn write_create_item_start(
        &mut self,
        element: quick_xml::events::BytesStart<'_>,
    ) -> Result<(), EwsError> {
        if self.create_item_seen || self.parents.last().map(Vec::as_slice) != Some(b"Body") {
            return Err(EwsError::InvalidResponse);
        }
        let mut element = element.into_owned();
        if has_attribute(&element, b"SendMeetingInvitations")? {
            return Err(EwsError::InvalidResponse);
        }
        element.push_attribute(("SendMeetingInvitations", "SendToNone"));
        self.writer
            .write_event(Event::Start(element))
            .map_err(|_| EwsError::InvalidResponse)?;
        self.create_item_seen = true;
        self.parents.push(b"CreateItem".to_vec());
        Ok(())
    }

    fn write_calendar_item_start(
        &mut self,
        element: quick_xml::events::BytesStart<'_>,
    ) -> Result<(), EwsError> {
        if !self.create_item_seen
            || self.calendar_item_seen
            || self.parents.last().map(Vec::as_slice) != Some(b"Items")
        {
            return Err(EwsError::InvalidResponse);
        }
        self.writer
            .write_event(Event::Start(element.into_owned()))
            .map_err(|_| EwsError::InvalidResponse)?;
        self.calendar_item_seen = true;
        self.parents.push(b"CalendarItem".to_vec());
        Ok(())
    }

    fn write_nested_start(
        &mut self,
        element: quick_xml::events::BytesStart<'_>,
    ) -> Result<(), EwsError> {
        let name = element.local_name().as_ref().to_vec();
        if self.parents.last().map(Vec::as_slice) == Some(b"CalendarItem")
            && matches!(name.as_slice(), b"Start" | b"End")
        {
            return Err(EwsError::InvalidResponse);
        }
        self.writer
            .write_event(Event::Start(element.into_owned()))
            .map_err(|_| EwsError::InvalidResponse)?;
        self.parents.push(name);
        Ok(())
    }

    fn write_end_event(
        &mut self,
        element: quick_xml::events::BytesEnd<'_>,
    ) -> Result<(), EwsError> {
        if element.local_name().as_ref() == b"Subject"
            && subject_belongs_to_calendar_item(&self.parents)
        {
            return self.write_subject_end(element);
        }
        if element.local_name().as_ref() == b"CalendarItem"
            && self.parents.last().map(Vec::as_slice) == Some(b"CalendarItem")
        {
            return self.write_calendar_item_end(element);
        }
        self.writer
            .write_event(Event::End(element.into_owned()))
            .map_err(|_| EwsError::InvalidResponse)?;
        self.parents.pop().ok_or(EwsError::InvalidResponse)?;
        Ok(())
    }

    fn write_subject_end(
        &mut self,
        element: quick_xml::events::BytesEnd<'_>,
    ) -> Result<(), EwsError> {
        if self.subject_seen {
            return Err(EwsError::InvalidResponse);
        }
        self.writer
            .write_event(Event::End(element.into_owned()))
            .map_err(|_| EwsError::InvalidResponse)?;
        self.subject_seen = true;
        self.parents.pop().ok_or(EwsError::InvalidResponse)?;
        Ok(())
    }

    fn write_calendar_item_end(
        &mut self,
        element: quick_xml::events::BytesEnd<'_>,
    ) -> Result<(), EwsError> {
        if !self.subject_seen || self.calendar_fields_written {
            return Err(EwsError::InvalidResponse);
        }
        write_text_element(&mut self.writer, "t:Start", &self.start)?;
        write_text_element(&mut self.writer, "t:End", &self.end)?;
        self.writer
            .write_event(Event::End(element.into_owned()))
            .map_err(|_| EwsError::InvalidResponse)?;
        self.parents.pop().ok_or(EwsError::InvalidResponse)?;
        self.calendar_fields_written = true;
        Ok(())
    }

    fn finish(self) -> Result<Vec<u8>, EwsError> {
        if !self.create_item_seen
            || !self.calendar_item_seen
            || !self.subject_seen
            || !self.calendar_fields_written
            || !self.parents.is_empty()
        {
            return Err(EwsError::InvalidResponse);
        }
        Ok(self.writer.into_inner())
    }
}

fn is_element(
    namespace: &ResolveResult<'_>,
    element: &quick_xml::events::BytesStart<'_>,
    expected_namespace: &[u8],
    expected_name: &[u8],
) -> bool {
    element.local_name().as_ref() == expected_name
        && matches!(namespace, ResolveResult::Bound(uri) if uri.as_ref() == expected_namespace)
}

fn subject_belongs_to_calendar_item(parents: &[Vec<u8>]) -> bool {
    parents.len() >= 2
        && parents[parents.len() - 2] == b"CalendarItem"
        && parents[parents.len() - 1] == b"Subject"
}

fn has_attribute(element: &BytesStart<'_>, name: &[u8]) -> Result<bool, EwsError> {
    for attribute in element.attributes().with_checks(true) {
        let attribute = attribute.map_err(|_| EwsError::InvalidResponse)?;
        if attribute.key.local_name().as_ref() == name {
            return Ok(true);
        }
    }
    Ok(false)
}

fn write_text_element(
    writer: &mut Writer<Vec<u8>>,
    name: &str,
    value: &str,
) -> Result<(), EwsError> {
    writer
        .write_event(Event::Start(BytesStart::new(name)))
        .map_err(|_| EwsError::InvalidResponse)?;
    writer
        .write_event(Event::Text(BytesText::new(value)))
        .map_err(|_| EwsError::InvalidResponse)?;
    writer
        .write_event(Event::End(BytesEnd::new(name)))
        .map_err(|_| EwsError::InvalidResponse)
}

pub(super) fn parse_create_item_response(
    document: &[u8],
) -> Result<EventId, CreateItemResponseError> {
    super::validate_calendar_operation_response(
        document,
        b"CreateItemResponse",
        b"CreateItemResponseMessage",
    )
    .map_err(|_| CreateItemResponseError::Unknown)?;
    let message = single_create_item_response(document)?;
    classify_create_item_response(&message)
}

fn single_create_item_response(
    document: &[u8],
) -> Result<CreateItemSoapResponseMessage, CreateItemResponseError> {
    let envelope: CreateItemSoapEnvelope =
        quick_xml::de::from_reader(document).map_err(|_| CreateItemResponseError::Unknown)?;
    let mut messages = envelope.body.create_item_response.response_messages.inner;
    if messages.len() != 1 {
        return Err(CreateItemResponseError::Unknown);
    }
    messages.pop().ok_or(CreateItemResponseError::Unknown)
}

fn classify_create_item_response(
    message: &CreateItemSoapResponseMessage,
) -> Result<EventId, CreateItemResponseError> {
    match (message.response_class.as_deref(), message.response_code) {
        (Some("Error"), Some(code))
            if code != ResponseCode::NoError && response_has_no_items(message) =>
        {
            Err(CreateItemResponseError::Rejected)
        }
        (Some("Success"), Some(ResponseCode::NoError)) => calendar_item_id(message),
        _ => Err(CreateItemResponseError::Unknown),
    }
}

#[cfg(test)]
mod tests;

fn response_has_no_items(message: &CreateItemSoapResponseMessage) -> bool {
    message
        .items
        .as_ref()
        .is_none_or(|items| items.inner.is_empty())
}

fn calendar_item_id(
    message: &CreateItemSoapResponseMessage,
) -> Result<EventId, CreateItemResponseError> {
    let [RealItem::CalendarItem(calendar_item)] = message
        .items
        .as_ref()
        .ok_or(CreateItemResponseError::Unknown)?
        .inner
        .as_slice()
    else {
        return Err(CreateItemResponseError::Unknown);
    };
    let item_id = calendar_item
        .item_id
        .as_ref()
        .ok_or(CreateItemResponseError::Unknown)?;
    validate_calendar_item_identity(&item_id.id, item_id.change_key.as_deref())?;
    EventId::try_from(item_id.id.clone()).map_err(|_| CreateItemResponseError::Unknown)
}

fn validate_calendar_item_identity(
    id: &str,
    change_key: Option<&str>,
) -> Result<(), CreateItemResponseError> {
    let change_key = change_key.ok_or(CreateItemResponseError::Unknown)?;
    if id.trim().is_empty()
        || id.chars().any(char::is_control)
        || change_key.trim().is_empty()
        || change_key.chars().any(char::is_control)
    {
        return Err(CreateItemResponseError::Unknown);
    }
    Ok(())
}

#[derive(Deserialize)]
struct CreateItemSoapEnvelope {
    #[serde(rename = "Body")]
    body: CreateItemSoapBody,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CreateItemSoapBody {
    create_item_response: CreateItemSoapResponse,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CreateItemSoapResponse {
    response_messages: CreateItemSoapResponseMessages,
}

#[derive(Deserialize)]
struct CreateItemSoapResponseMessages {
    #[serde(rename = "$value", default)]
    inner: Vec<CreateItemSoapResponseMessage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CreateItemSoapResponseMessage {
    #[serde(rename = "@ResponseClass")]
    response_class: Option<String>,
    response_code: Option<ResponseCode>,
    items: Option<ews::Items>,
}
