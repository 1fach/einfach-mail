use einfach_mail_application::CalendarOperationError;
use einfach_mail_domain::{CalendarEventCreation, EventId};
use ews::{
    BaseFolderId, BasePoint, BaseShape, ItemShape, RealItem, View,
    find_item::{FindItem, RootFolder, Traversal},
    response::ResponseCode,
};
use quick_xml::{
    NsReader, Writer,
    events::{BytesEnd, BytesStart, Event},
    name::ResolveResult,
};
use serde::Deserialize;

use crate::{
    error::EwsError,
    transport::{EwsRequestSequence, serialize_soap_request},
};

use super::{
    EWS_MESSAGES_NAMESPACE,
    create::load_confirmed_event,
    item::{OPERATION_MARKER_PROPERTY_NAME, OPERATION_MARKER_PROPERTY_SET_ID},
};

pub(super) async fn recover_event_creation(
    sequence: &EwsRequestSequence<'_>,
    creation: &CalendarEventCreation,
) -> Result<einfach_mail_domain::CalendarOccurrence, CalendarOperationError> {
    let request = find_item_request(creation).map_err(|_| unknown_creation_outcome())?;
    let response = sequence
        .execute_xml_read(&request)
        .await
        .map_err(|_| unknown_creation_outcome())?;
    let event_id = parse_find_item_candidate(&response).map_err(|_| unknown_creation_outcome())?;
    load_confirmed_event(sequence, creation, &event_id).await
}

fn unknown_creation_outcome() -> CalendarOperationError {
    CalendarOperationError::CreationOutcomeUnknown
}

pub(super) fn find_item_request(creation: &CalendarEventCreation) -> Result<Vec<u8>, EwsError> {
    let request = FindItem {
        traversal: Traversal::Shallow,
        item_shape: ItemShape {
            base_shape: BaseShape::IdOnly,
            include_mime_content: Some(false),
            additional_properties: None,
        },
        view: Some(View::IndexedPageItemView {
            max_entries_returned: Some(2),
            offset: 0,
            base_point: BasePoint::Beginning,
        }),
        parent_folder_ids: vec![BaseFolderId::new_distinguished("calendar")],
    };
    let document = serialize_soap_request(request)?;
    add_marker_restriction(&document, &creation.id().as_uuid().to_string())
}

fn add_marker_restriction(document: &[u8], operation_id: &str) -> Result<Vec<u8>, EwsError> {
    let mut reader = NsReader::from_reader(document);
    let mut transform = FindItemRestrictionTransform::new(document.len(), operation_id);
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

struct FindItemRestrictionTransform {
    writer: Writer<Vec<u8>>,
    parents: Vec<Vec<u8>>,
    operation_id: String,
    find_item_seen: bool,
    folder_ids_seen: bool,
}

impl FindItemRestrictionTransform {
    fn new(document_len: usize, operation_id: &str) -> Self {
        Self {
            writer: Writer::new(Vec::with_capacity(document_len + 256)),
            parents: Vec::new(),
            operation_id: operation_id.to_owned(),
            find_item_seen: false,
            folder_ids_seen: false,
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
        element: BytesStart<'_>,
    ) -> Result<(), EwsError> {
        if is_element(namespace, &element, EWS_MESSAGES_NAMESPACE, b"FindItem") {
            return self.write_find_item_start(element);
        }
        if is_element(
            namespace,
            &element,
            EWS_MESSAGES_NAMESPACE,
            b"ParentFolderIds",
        ) {
            return self.write_parent_folder_ids_start(element);
        }
        self.write_nested_start(element)
    }

    fn write_find_item_start(&mut self, element: BytesStart<'_>) -> Result<(), EwsError> {
        if self.find_item_seen || self.parents.last().map(Vec::as_slice) != Some(b"Body") {
            return Err(EwsError::InvalidResponse);
        }
        self.write_start_element(element, b"FindItem")?;
        self.find_item_seen = true;
        Ok(())
    }

    fn write_parent_folder_ids_start(&mut self, element: BytesStart<'_>) -> Result<(), EwsError> {
        if !self.find_item_seen
            || self.folder_ids_seen
            || self.parents.last().map(Vec::as_slice) != Some(b"FindItem")
        {
            return Err(EwsError::InvalidResponse);
        }
        write_marker_restriction(&mut self.writer, &self.operation_id)?;
        self.write_start_element(element, b"ParentFolderIds")?;
        self.folder_ids_seen = true;
        Ok(())
    }

    fn write_nested_start(&mut self, element: BytesStart<'_>) -> Result<(), EwsError> {
        let name = element.local_name().as_ref().to_vec();
        self.writer
            .write_event(Event::Start(element.into_owned()))
            .map_err(|_| EwsError::InvalidResponse)?;
        self.parents.push(name);
        Ok(())
    }

    fn write_start_element(
        &mut self,
        element: BytesStart<'_>,
        name: &[u8],
    ) -> Result<(), EwsError> {
        self.writer
            .write_event(Event::Start(element.into_owned()))
            .map_err(|_| EwsError::InvalidResponse)?;
        self.parents.push(name.to_vec());
        Ok(())
    }

    fn write_end_event(&mut self, element: BytesEnd<'_>) -> Result<(), EwsError> {
        self.writer
            .write_event(Event::End(element.into_owned()))
            .map_err(|_| EwsError::InvalidResponse)?;
        self.parents.pop().ok_or(EwsError::InvalidResponse)?;
        Ok(())
    }

    fn finish(self) -> Result<Vec<u8>, EwsError> {
        if !self.find_item_seen || !self.folder_ids_seen || !self.parents.is_empty() {
            return Err(EwsError::InvalidResponse);
        }
        Ok(self.writer.into_inner())
    }
}

fn write_marker_restriction(
    writer: &mut Writer<Vec<u8>>,
    operation_id: &str,
) -> Result<(), EwsError> {
    write_start(writer, "Restriction")?;
    write_start(writer, "t:IsEqualTo")?;
    write_operation_marker_field(writer)?;
    write_start(writer, "t:FieldURIOrConstant")?;
    write_operation_marker_value(writer, operation_id)?;
    write_end(writer, "t:FieldURIOrConstant")?;
    write_end(writer, "t:IsEqualTo")?;
    write_end(writer, "Restriction")
}

fn write_operation_marker_field(writer: &mut Writer<Vec<u8>>) -> Result<(), EwsError> {
    let mut extended_field = BytesStart::new("t:ExtendedFieldURI");
    extended_field.push_attribute(("PropertySetId", OPERATION_MARKER_PROPERTY_SET_ID));
    extended_field.push_attribute(("PropertyName", OPERATION_MARKER_PROPERTY_NAME));
    extended_field.push_attribute(("PropertyType", "String"));
    writer
        .write_event(Event::Empty(extended_field))
        .map_err(|_| EwsError::InvalidResponse)
}

fn write_operation_marker_value(
    writer: &mut Writer<Vec<u8>>,
    operation_id: &str,
) -> Result<(), EwsError> {
    let mut constant = BytesStart::new("t:Constant");
    constant.push_attribute(("Value", operation_id));
    writer
        .write_event(Event::Empty(constant))
        .map_err(|_| EwsError::InvalidResponse)
}

fn write_start(writer: &mut Writer<Vec<u8>>, name: &str) -> Result<(), EwsError> {
    writer
        .write_event(Event::Start(BytesStart::new(name)))
        .map_err(|_| EwsError::InvalidResponse)
}

fn write_end(writer: &mut Writer<Vec<u8>>, name: &str) -> Result<(), EwsError> {
    writer
        .write_event(Event::End(BytesEnd::new(name)))
        .map_err(|_| EwsError::InvalidResponse)
}

fn is_element(
    namespace: &ResolveResult<'_>,
    element: &BytesStart<'_>,
    expected_namespace: &[u8],
    expected_name: &[u8],
) -> bool {
    element.local_name().as_ref() == expected_name
        && matches!(namespace, ResolveResult::Bound(uri) if uri.as_ref() == expected_namespace)
}

pub(super) fn parse_find_item_candidate(document: &[u8]) -> Result<EventId, EwsError> {
    super::validate_calendar_operation_response(
        document,
        b"FindItemResponse",
        b"FindItemResponseMessage",
    )?;
    parse_find_item_response(document)
}

fn parse_find_item_response(document: &[u8]) -> Result<EventId, EwsError> {
    let message = single_find_item_response(document)?;
    if message.response_class.as_deref() != Some("Success")
        || message.response_code != Some(ResponseCode::NoError)
    {
        return Err(EwsError::InvalidResponse);
    }
    find_item_candidate(
        message
            .root_folder
            .as_ref()
            .ok_or(EwsError::InvalidResponse)?,
    )
}

fn single_find_item_response(document: &[u8]) -> Result<FindItemSoapResponseMessage, EwsError> {
    let envelope: FindItemSoapEnvelope =
        quick_xml::de::from_reader(document).map_err(|_| EwsError::InvalidResponse)?;
    let mut messages = envelope.body.find_item_response.response_messages.inner;
    if messages.len() != 1 {
        return Err(EwsError::InvalidResponse);
    }
    messages.pop().ok_or(EwsError::InvalidResponse)
}

#[cfg(test)]
mod tests;

fn find_item_candidate(root_folder: &RootFolder) -> Result<EventId, EwsError> {
    if root_folder.includes_last_item_in_range != Some(true)
        || root_folder
            .total_items_in_view
            .is_some_and(|total| total != root_folder.items.inner.len())
    {
        return Err(EwsError::InvalidResponse);
    }
    let [RealItem::CalendarItem(calendar_item)] = root_folder.items.inner.as_slice() else {
        return Err(EwsError::InvalidResponse);
    };
    let id = calendar_item
        .item_id
        .as_ref()
        .map(|item_id| item_id.id.as_str())
        .filter(|id| !id.trim().is_empty() && !id.chars().any(char::is_control))
        .ok_or(EwsError::InvalidResponse)?;
    EventId::try_from(id.to_owned()).map_err(|_| EwsError::InvalidResponse)
}

#[derive(Deserialize)]
struct FindItemSoapEnvelope {
    #[serde(rename = "Body")]
    body: FindItemSoapBody,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct FindItemSoapBody {
    find_item_response: FindItemSoapResponse,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct FindItemSoapResponse {
    response_messages: FindItemSoapResponseMessages,
}

#[derive(Deserialize)]
struct FindItemSoapResponseMessages {
    #[serde(rename = "$value", default)]
    inner: Vec<FindItemSoapResponseMessage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct FindItemSoapResponseMessage {
    #[serde(rename = "@ResponseClass")]
    response_class: Option<String>,
    response_code: Option<ResponseCode>,
    root_folder: Option<RootFolder>,
}
