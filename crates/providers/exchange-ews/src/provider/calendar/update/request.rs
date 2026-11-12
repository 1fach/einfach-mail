use einfach_mail_domain::{CalendarEventUpdate, CalendarOccurrenceTime};
use ews::{
    BaseItemId, Message, PathToElement,
    update_item::{
        ConflictResolution, ItemChange, ItemChangeDescription, ItemChangeInner, UpdateItem, Updates,
    },
};
use quick_xml::{
    NsReader, Writer, XmlVersion,
    encoding::Decoder,
    events::{BytesEnd, BytesStart, BytesText, Event},
    name::ResolveResult,
};

use super::super::{EWS_MESSAGES_NAMESPACE, EWS_TYPES_NAMESPACE, format_ews_datetime};
use crate::{error::EwsError, transport::serialize_soap_request};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UpdateField {
    Subject,
    Start,
    End,
}

impl UpdateField {
    fn from_uri(uri: &str) -> Option<Self> {
        match uri {
            "item:Subject" => Some(Self::Subject),
            "calendar:Start" => Some(Self::Start),
            "calendar:End" => Some(Self::End),
            _ => None,
        }
    }

    fn uri(self) -> &'static str {
        match self {
            Self::Subject => "item:Subject",
            Self::Start => "calendar:Start",
            Self::End => "calendar:End",
        }
    }
}

struct UpdateFields {
    subject: Option<String>,
    start: Option<String>,
    end: Option<String>,
}

impl UpdateFields {
    fn from_update(update: &CalendarEventUpdate) -> Result<Self, EwsError> {
        let source = update.source().occurrence();
        let subject = (source.subject() != Some(update.draft().subject()))
            .then(|| update.draft().subject().to_owned());
        let (start, end) = changed_time_fields(update)?;
        let fields = Self {
            subject,
            start,
            end,
        };
        if fields.expected().is_empty() {
            return Err(EwsError::InvalidResponse);
        }
        Ok(fields)
    }

    fn expected(&self) -> Vec<UpdateField> {
        let mut fields = Vec::with_capacity(3);
        if self.subject.is_some() {
            fields.push(UpdateField::Subject);
        }
        if self.start.is_some() && self.end.is_some() {
            fields.extend([UpdateField::Start, UpdateField::End]);
        }
        fields
    }

    fn description(&self, field: UpdateField) -> ItemChangeDescription {
        let message = match field {
            UpdateField::Subject => Message {
                subject: self.subject.clone(),
                ..Message::default()
            },
            UpdateField::Start | UpdateField::End => Message::default(),
        };
        ItemChangeDescription::SetItemField {
            field_uri: PathToElement::FieldURI {
                field_URI: field.uri().to_owned(),
            },
            message,
        }
    }
}

fn changed_time_fields(
    update: &CalendarEventUpdate,
) -> Result<(Option<String>, Option<String>), EwsError> {
    if update.source().occurrence().time() == update.draft().time() {
        return Ok((None, None));
    }
    let CalendarOccurrenceTime::Timed { start, end } = update.draft().time() else {
        return Err(EwsError::InvalidResponse);
    };
    Ok((
        Some(format_ews_datetime(*start)?),
        Some(format_ews_datetime(*end)?),
    ))
}

pub(super) fn update_item_request(
    update: &CalendarEventUpdate,
    change_key: &str,
) -> Result<Vec<u8>, EwsError> {
    let fields = UpdateFields::from_update(update)?;
    let expected_fields = fields.expected();
    let operation = build_update_operation(update, change_key, &fields, &expected_fields);
    let document = serialize_soap_request(operation)?;
    add_update_item_fields(&document, expected_fields, fields.start, fields.end)
}

fn build_update_operation(
    update: &CalendarEventUpdate,
    change_key: &str,
    fields: &UpdateFields,
    expected_fields: &[UpdateField],
) -> UpdateItem {
    UpdateItem {
        message_disposition: ews::MessageDisposition::SaveOnly,
        conflict_resolution: Some(ConflictResolution::NeverOverwrite),
        item_changes: vec![build_item_change(
            update,
            change_key,
            fields,
            expected_fields,
        )],
    }
}

fn build_item_change(
    update: &CalendarEventUpdate,
    change_key: &str,
    fields: &UpdateFields,
    expected_fields: &[UpdateField],
) -> ItemChange {
    let item_id = build_item_id(update, change_key);
    let updates = build_item_updates(fields, expected_fields);
    ItemChange {
        item_change: ItemChangeInner { item_id, updates },
    }
}

fn build_item_id(update: &CalendarEventUpdate, change_key: &str) -> BaseItemId {
    BaseItemId::ItemId {
        id: update.source().occurrence().id().as_str().to_owned(),
        change_key: Some(change_key.to_owned()),
    }
}

fn build_item_updates(fields: &UpdateFields, expected_fields: &[UpdateField]) -> Updates {
    Updates {
        inner: expected_fields
            .iter()
            .map(|field| fields.description(*field))
            .collect(),
    }
}

fn add_update_item_fields(
    document: &[u8],
    expected_fields: Vec<UpdateField>,
    start: Option<String>,
    end: Option<String>,
) -> Result<Vec<u8>, EwsError> {
    let mut reader = NsReader::from_reader(document);
    let decoder = reader.decoder();
    let mut transform = UpdateItemXmlTransform::new(document.len(), expected_fields, start, end);
    loop {
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|_| EwsError::InvalidResponse)?;
        if matches!(&event, Event::Eof) {
            break;
        }
        transform.write_event(&namespace, event, decoder)?;
    }
    transform.finish()
}

struct UpdateItemXmlTransform {
    writer: Writer<Vec<u8>>,
    parents: Vec<Vec<u8>>,
    expected_fields: Vec<UpdateField>,
    seen_fields: Vec<UpdateField>,
    active_field: Option<UpdateField>,
    start: Option<String>,
    end: Option<String>,
    update_item_seen: bool,
    field_message_seen: bool,
    field_message_complete: bool,
    subject_seen: bool,
}

enum UpdateXmlElement {
    UpdateItem,
    SetItemField,
    FieldUri,
    Message,
    Subject,
    Other,
}

impl UpdateItemXmlTransform {
    fn new(
        document_len: usize,
        expected_fields: Vec<UpdateField>,
        start: Option<String>,
        end: Option<String>,
    ) -> Self {
        Self {
            writer: Writer::new(Vec::with_capacity(document_len + 128)),
            parents: Vec::new(),
            expected_fields,
            seen_fields: Vec::new(),
            active_field: None,
            start,
            end,
            update_item_seen: false,
            field_message_seen: false,
            field_message_complete: false,
            subject_seen: false,
        }
    }

    fn write_event(
        &mut self,
        namespace: &ResolveResult<'_>,
        event: Event<'_>,
        decoder: Decoder,
    ) -> Result<(), EwsError> {
        match event {
            Event::Start(element) => self.write_start(namespace, element, decoder),
            Event::Empty(element) => self.write_empty(namespace, element, decoder),
            Event::End(element) => self.write_end(element),
            event => self
                .writer
                .write_event(event.into_owned())
                .map_err(|_| EwsError::InvalidResponse),
        }
    }

    fn parent_is(&self, name: &[u8]) -> bool {
        self.parents.last().map(Vec::as_slice) == Some(name)
    }

    fn start_element_kind(
        &self,
        namespace: &ResolveResult<'_>,
        element: &BytesStart<'_>,
    ) -> UpdateXmlElement {
        match classify_element(namespace, element) {
            UpdateXmlElement::FieldUri if self.parent_is(b"SetItemField") => {
                UpdateXmlElement::FieldUri
            }
            UpdateXmlElement::Subject
                if self.active_field == Some(UpdateField::Subject)
                    && self.parent_is(b"CalendarItem") =>
            {
                UpdateXmlElement::Subject
            }
            UpdateXmlElement::FieldUri | UpdateXmlElement::Subject => UpdateXmlElement::Other,
            element_kind => element_kind,
        }
    }

    fn write_start(
        &mut self,
        namespace: &ResolveResult<'_>,
        element: BytesStart<'_>,
        decoder: Decoder,
    ) -> Result<(), EwsError> {
        match self.start_element_kind(namespace, &element) {
            UpdateXmlElement::UpdateItem => self.write_update_item_start(element),
            UpdateXmlElement::SetItemField => self.write_set_item_field_start(element),
            UpdateXmlElement::FieldUri => self.write_field_uri_start(element, decoder),
            UpdateXmlElement::Message => self.write_calendar_item_start(element),
            UpdateXmlElement::Subject => self.write_subject_start(element),
            _ => self.write_nested_start(element),
        }
    }

    fn write_empty(
        &mut self,
        namespace: &ResolveResult<'_>,
        element: BytesStart<'_>,
        decoder: Decoder,
    ) -> Result<(), EwsError> {
        match classify_element(namespace, &element) {
            UpdateXmlElement::FieldUri if self.parent_is(b"SetItemField") => {
                self.write_field_uri_empty(element, decoder)
            }
            UpdateXmlElement::Message => self.write_calendar_item_empty(),
            UpdateXmlElement::UpdateItem => Err(EwsError::InvalidResponse),
            _ => self
                .writer
                .write_event(Event::Empty(element.into_owned()))
                .map_err(|_| EwsError::InvalidResponse),
        }
    }

    fn write_update_item_start(&mut self, element: BytesStart<'_>) -> Result<(), EwsError> {
        if self.update_item_seen || !self.parent_is(b"Body") {
            return Err(EwsError::InvalidResponse);
        }
        let mut element = element.into_owned();
        if has_attribute(&element, b"SendMeetingInvitationsOrCancellations")? {
            return Err(EwsError::InvalidResponse);
        }
        element.push_attribute(("SendMeetingInvitationsOrCancellations", "SendToNone"));
        self.writer
            .write_event(Event::Start(element))
            .map_err(|_| EwsError::InvalidResponse)?;
        self.update_item_seen = true;
        self.parents.push(b"UpdateItem".to_vec());
        Ok(())
    }

    fn write_set_item_field_start(&mut self, element: BytesStart<'_>) -> Result<(), EwsError> {
        if !self.parent_is(b"Updates") || self.active_field.is_some() {
            return Err(EwsError::InvalidResponse);
        }
        self.field_message_seen = false;
        self.field_message_complete = false;
        self.subject_seen = false;
        self.write_nested_start(element)
    }

    fn write_field_uri_start(
        &mut self,
        element: BytesStart<'_>,
        decoder: Decoder,
    ) -> Result<(), EwsError> {
        self.begin_field(&element, decoder)?;
        self.write_nested_start(element)
    }

    fn write_field_uri_empty(
        &mut self,
        element: BytesStart<'_>,
        decoder: Decoder,
    ) -> Result<(), EwsError> {
        self.begin_field(&element, decoder)?;
        self.writer
            .write_event(Event::Empty(element.into_owned()))
            .map_err(|_| EwsError::InvalidResponse)
    }

    fn begin_field(&mut self, element: &BytesStart<'_>, decoder: Decoder) -> Result<(), EwsError> {
        if self.active_field.is_some() {
            return Err(EwsError::InvalidResponse);
        }
        let field = field_uri(element, decoder)?;
        if self.expected_fields.get(self.seen_fields.len()) != Some(&field) {
            return Err(EwsError::InvalidResponse);
        }
        self.seen_fields.push(field);
        self.active_field = Some(field);
        Ok(())
    }

    fn write_calendar_item_start(&mut self, element: BytesStart<'_>) -> Result<(), EwsError> {
        if !self.parent_is(b"SetItemField")
            || self.active_field.is_none()
            || self.field_message_seen
            || has_any_attribute(&element)?
        {
            return Err(EwsError::InvalidResponse);
        }
        self.field_message_seen = true;
        self.writer
            .write_event(Event::Start(BytesStart::new("t:CalendarItem")))
            .map_err(|_| EwsError::InvalidResponse)?;
        self.parents.push(b"CalendarItem".to_vec());
        Ok(())
    }

    fn write_calendar_item_empty(&mut self) -> Result<(), EwsError> {
        if !self.parent_is(b"SetItemField")
            || self.active_field.is_none()
            || self.field_message_seen
        {
            return Err(EwsError::InvalidResponse);
        }
        self.field_message_seen = true;
        self.writer
            .write_event(Event::Start(BytesStart::new("t:CalendarItem")))
            .map_err(|_| EwsError::InvalidResponse)?;
        self.write_calendar_item_fields()?;
        self.writer
            .write_event(Event::End(BytesEnd::new("t:CalendarItem")))
            .map_err(|_| EwsError::InvalidResponse)?;
        self.field_message_complete = true;
        Ok(())
    }

    fn write_subject_start(&mut self, element: BytesStart<'_>) -> Result<(), EwsError> {
        if self.subject_seen {
            return Err(EwsError::InvalidResponse);
        }
        self.subject_seen = true;
        self.write_nested_start(element)
    }

    fn write_nested_start(&mut self, element: BytesStart<'_>) -> Result<(), EwsError> {
        let name = element.local_name().as_ref().to_vec();
        self.writer
            .write_event(Event::Start(element.into_owned()))
            .map_err(|_| EwsError::InvalidResponse)?;
        self.parents.push(name);
        Ok(())
    }

    fn write_end(&mut self, element: BytesEnd<'_>) -> Result<(), EwsError> {
        match element.local_name().as_ref() {
            b"Message" if self.parent_is(b"CalendarItem") => self.write_calendar_item_end(),
            b"SetItemField" => self.write_set_item_field_end(element),
            _ => {
                self.writer
                    .write_event(Event::End(element.into_owned()))
                    .map_err(|_| EwsError::InvalidResponse)?;
                self.parents.pop().ok_or(EwsError::InvalidResponse)?;
                Ok(())
            }
        }
    }

    fn write_calendar_item_end(&mut self) -> Result<(), EwsError> {
        if !self.field_message_seen || self.field_message_complete {
            return Err(EwsError::InvalidResponse);
        }
        self.write_calendar_item_fields()?;
        self.writer
            .write_event(Event::End(BytesEnd::new("t:CalendarItem")))
            .map_err(|_| EwsError::InvalidResponse)?;
        self.parents.pop().ok_or(EwsError::InvalidResponse)?;
        self.field_message_complete = true;
        Ok(())
    }

    fn write_calendar_item_fields(&mut self) -> Result<(), EwsError> {
        match self.active_field {
            Some(UpdateField::Subject) if self.subject_seen => Ok(()),
            Some(UpdateField::Start) => write_text_element(
                &mut self.writer,
                "t:Start",
                self.start.as_deref().ok_or(EwsError::InvalidResponse)?,
            ),
            Some(UpdateField::End) => write_text_element(
                &mut self.writer,
                "t:End",
                self.end.as_deref().ok_or(EwsError::InvalidResponse)?,
            ),
            _ => Err(EwsError::InvalidResponse),
        }
    }

    fn write_set_item_field_end(&mut self, element: BytesEnd<'_>) -> Result<(), EwsError> {
        if !self.parent_is(b"SetItemField")
            || self.active_field.is_none()
            || !self.field_message_complete
        {
            return Err(EwsError::InvalidResponse);
        }
        self.writer
            .write_event(Event::End(element.into_owned()))
            .map_err(|_| EwsError::InvalidResponse)?;
        self.parents.pop().ok_or(EwsError::InvalidResponse)?;
        self.active_field = None;
        self.field_message_seen = false;
        self.field_message_complete = false;
        Ok(())
    }

    fn finish(self) -> Result<Vec<u8>, EwsError> {
        if !self.update_item_seen
            || self.seen_fields != self.expected_fields
            || self.active_field.is_some()
            || !self.parents.is_empty()
            || self.start.is_some() != self.seen_fields.contains(&UpdateField::Start)
            || self.end.is_some() != self.seen_fields.contains(&UpdateField::End)
        {
            return Err(EwsError::InvalidResponse);
        }
        Ok(self.writer.into_inner())
    }
}

fn classify_element(namespace: &ResolveResult<'_>, element: &BytesStart<'_>) -> UpdateXmlElement {
    if is_element(namespace, element, EWS_MESSAGES_NAMESPACE, b"UpdateItem") {
        UpdateXmlElement::UpdateItem
    } else if is_element(namespace, element, EWS_TYPES_NAMESPACE, b"SetItemField") {
        UpdateXmlElement::SetItemField
    } else if is_element(namespace, element, EWS_TYPES_NAMESPACE, b"FieldURI") {
        UpdateXmlElement::FieldUri
    } else if is_element(namespace, element, EWS_TYPES_NAMESPACE, b"Message") {
        UpdateXmlElement::Message
    } else if is_element(namespace, element, EWS_TYPES_NAMESPACE, b"Subject") {
        UpdateXmlElement::Subject
    } else {
        UpdateXmlElement::Other
    }
}

fn field_uri(element: &BytesStart<'_>, decoder: Decoder) -> Result<UpdateField, EwsError> {
    let mut field_uri = None;
    for attribute in element.attributes().with_checks(true) {
        let attribute = attribute.map_err(|_| EwsError::InvalidResponse)?;
        if attribute.key.as_ref() == b"FieldURI" {
            field_uri = Some(
                attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder)
                    .map_err(|_| EwsError::InvalidResponse)?,
            );
        }
    }
    field_uri
        .as_deref()
        .and_then(UpdateField::from_uri)
        .ok_or(EwsError::InvalidResponse)
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

fn has_any_attribute(element: &BytesStart<'_>) -> Result<bool, EwsError> {
    element
        .attributes()
        .with_checks(true)
        .next()
        .transpose()
        .map(|attribute| attribute.is_some())
        .map_err(|_| EwsError::InvalidResponse)
}

fn is_element(
    namespace: &ResolveResult<'_>,
    element: &BytesStart<'_>,
    expected_namespace: &[u8],
    expected_name: &[u8],
) -> bool {
    element.local_name().as_ref() == expected_name
        && matches!(namespace, ResolveResult::Bound(actual) if actual.as_ref() == expected_namespace)
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
