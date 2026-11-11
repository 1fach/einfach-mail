use einfach_mail_domain::{MessageId, ReadState};
use ews::{
    BaseItemId, BaseShape, ItemShape, Message, MessageDisposition, PathToElement, ResponseClass,
    get_item::{GetItem, GetItemResponseMessage},
    response::ResponseCode,
    server_version::ServerVersionInfo,
    update_item::UpdateItemResponseMessage,
    update_item::{
        ConflictResolution, ItemChange, ItemChangeDescription, ItemChangeInner, UpdateItem, Updates,
    },
};
use quick_xml::{NsReader, Writer, de::from_reader, events::Event, name::ResolveResult};
use serde::Deserialize;

use crate::{
    error::EwsError,
    transport::{EwsSoapResponse, parse_soap_response, serialize_soap_request},
};

const EWS_MESSAGES_NAMESPACE: &[u8] =
    b"http://schemas.microsoft.com/exchange/services/2006/messages";

pub(crate) struct CurrentEwsItemVersion {
    id: String,
    change_key: String,
}

impl CurrentEwsItemVersion {
    pub(crate) fn from_message(
        message: &Message,
        requested_id: &MessageId,
    ) -> Result<Self, EwsError> {
        let item_id = message.item_id.as_ref().ok_or(EwsError::InvalidResponse)?;
        let change_key = item_id
            .change_key
            .as_deref()
            .filter(|change_key| !change_key.is_empty())
            .ok_or(EwsError::InvalidResponse)?;
        if item_id.id != requested_id.as_str() {
            return Err(EwsError::InvalidResponse);
        }

        Ok(Self {
            id: item_id.id.clone(),
            change_key: change_key.to_owned(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UpdateConfirmationError {
    Rejected(ResponseCode),
    Unknown,
}

pub(crate) struct ParsedUpdateItemResponse {
    response: EwsSoapResponse<UpdateItemResponseMessage>,
    response_codes: Vec<Option<ResponseCode>>,
}

pub(crate) struct ParsedGetItemResponse {
    response: EwsSoapResponse<GetItemResponseMessage>,
    response_codes: Vec<Option<ResponseCode>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GetItemError {
    Rejected(ResponseCode),
    InvalidResponse,
}

pub(crate) fn get_item_request(message_id: &MessageId) -> GetItem {
    GetItem {
        item_shape: ItemShape {
            base_shape: BaseShape::IdOnly,
            include_mime_content: Some(false),
            additional_properties: None,
        },
        item_ids: vec![BaseItemId::ItemId {
            id: message_id.as_str().to_owned(),
            change_key: None,
        }],
    }
}

pub(crate) fn parse_get_item_response(document: &[u8]) -> Result<ParsedGetItemResponse, EwsError> {
    let response = parse_soap_response::<GetItem>(document)?;
    let response_codes: GetItemSoapEnvelope =
        from_reader(document).map_err(|_| EwsError::InvalidResponse)?;
    let response_codes = response_codes
        .body
        .get_item_response
        .response_messages
        .inner;
    if response.messages.len() != response_codes.len() {
        return Err(EwsError::InvalidResponse);
    }
    Ok(ParsedGetItemResponse {
        response,
        response_codes: response_codes
            .into_iter()
            .map(|message| message.response_code)
            .collect(),
    })
}

pub(crate) fn current_item_version(
    response: ParsedGetItemResponse,
    requested_id: &MessageId,
) -> Result<(Option<ServerVersionInfo>, CurrentEwsItemVersion), GetItemError> {
    let EwsSoapResponse {
        messages,
        server_version,
    } = response.response;
    let [response_message] = messages.as_slice() else {
        return Err(GetItemError::InvalidResponse);
    };
    let [Some(response_code)] = response.response_codes.as_slice() else {
        return Err(GetItemError::InvalidResponse);
    };
    match (response_message, response_code) {
        (ResponseClass::Success(response), ResponseCode::NoError) => {
            let [ews::RealItem::Message(message)] = response.items.inner.as_slice() else {
                return Err(GetItemError::InvalidResponse);
            };
            let version = CurrentEwsItemVersion::from_message(message, requested_id)
                .map_err(|_| GetItemError::InvalidResponse)?;
            Ok((server_version, version))
        }
        (ResponseClass::Error(error), response_code)
            if *response_code != ResponseCode::NoError && error.response_code == *response_code =>
        {
            Err(GetItemError::Rejected(*response_code))
        }
        _ => Err(GetItemError::InvalidResponse),
    }
}

pub(crate) fn serialize_update_item_request(
    current: &CurrentEwsItemVersion,
    requested: ReadState,
) -> Result<Vec<u8>, EwsError> {
    let is_read = matches!(requested, ReadState::Read);
    let request = UpdateItem {
        message_disposition: MessageDisposition::SaveOnly,
        conflict_resolution: Some(ConflictResolution::NeverOverwrite),
        item_changes: vec![ItemChange {
            item_change: ItemChangeInner {
                item_id: BaseItemId::ItemId {
                    id: current.id.clone(),
                    change_key: Some(current.change_key.clone()),
                },
                updates: Updates {
                    inner: vec![ItemChangeDescription::SetItemField {
                        field_uri: PathToElement::FieldURI {
                            field_URI: "message:IsRead".to_owned(),
                        },
                        message: Message {
                            is_read: Some(is_read),
                            ..Message::default()
                        },
                    }],
                },
            },
        }],
    };
    add_suppress_read_receipts(&serialize_soap_request(request)?)
}

pub(crate) fn confirm_update(
    response: ParsedUpdateItemResponse,
    requested_id: &MessageId,
    requested: ReadState,
) -> Result<ReadState, UpdateConfirmationError> {
    let ParsedUpdateItemResponse {
        response,
        response_codes,
    } = response;
    let [response_message] = response.messages.as_slice() else {
        return Err(UpdateConfirmationError::Unknown);
    };
    let [Some(response_code)] = response_codes.as_slice() else {
        return Err(UpdateConfirmationError::Unknown);
    };
    match (response_message, response_code) {
        (ResponseClass::Success(response), ResponseCode::NoError) => {
            let [item] = response.items.inner.as_slice() else {
                return Err(UpdateConfirmationError::Unknown);
            };
            let ews::RealItem::Message(item) = item else {
                return Err(UpdateConfirmationError::Unknown);
            };
            let Some(item_id) = item.item_id.as_ref() else {
                return Err(UpdateConfirmationError::Unknown);
            };
            if item_id.id != requested_id.as_str()
                || item_id.change_key.as_deref().is_none_or(str::is_empty)
            {
                return Err(UpdateConfirmationError::Unknown);
            }
            Ok(requested)
        }
        (ResponseClass::Error(error), response_code)
            if *response_code != ResponseCode::NoError && error.response_code == *response_code =>
        {
            Err(UpdateConfirmationError::Rejected(*response_code))
        }
        (ResponseClass::Success(_) | ResponseClass::Error(_) | ResponseClass::Warning(_), _) => {
            Err(UpdateConfirmationError::Unknown)
        }
    }
}

pub(crate) fn parse_update_item_response(
    document: &[u8],
) -> Result<ParsedUpdateItemResponse, EwsError> {
    let response = parse_soap_response::<UpdateItem>(document)?;
    let response_codes: UpdateItemSoapEnvelope =
        from_reader(document).map_err(|_| EwsError::InvalidResponse)?;
    let response_codes = response_codes
        .body
        .update_item_response
        .response_messages
        .inner;
    if response.messages.len() != response_codes.len() {
        return Err(EwsError::InvalidResponse);
    }
    Ok(ParsedUpdateItemResponse {
        response,
        response_codes: response_codes
            .into_iter()
            .map(|message| message.response_code)
            .collect(),
    })
}

#[derive(Deserialize)]
struct GetItemSoapEnvelope {
    #[serde(rename = "Body")]
    body: GetItemSoapBody,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct GetItemSoapBody {
    get_item_response: GetItemSoapResponse,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct GetItemSoapResponse {
    response_messages: GetItemSoapResponseMessages,
}

#[derive(Deserialize)]
struct GetItemSoapResponseMessages {
    #[serde(rename = "$value", default)]
    inner: Vec<GetItemSoapResponseMessage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct GetItemSoapResponseMessage {
    response_code: Option<ResponseCode>,
}

#[derive(Deserialize)]
struct UpdateItemSoapEnvelope {
    #[serde(rename = "Body")]
    body: UpdateItemSoapBody,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UpdateItemSoapBody {
    update_item_response: UpdateItemSoapResponse,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UpdateItemSoapResponse {
    response_messages: UpdateItemSoapResponseMessages,
}

#[derive(Deserialize)]
struct UpdateItemSoapResponseMessages {
    #[serde(rename = "$value", default)]
    inner: Vec<UpdateItemSoapResponseMessage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UpdateItemSoapResponseMessage {
    response_code: Option<ResponseCode>,
}

fn add_suppress_read_receipts(document: &[u8]) -> Result<Vec<u8>, EwsError> {
    let mut reader = NsReader::from_reader(document);
    let mut writer = Writer::new(Vec::with_capacity(document.len() + 32));
    let mut update_item_seen = false;

    loop {
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|_| EwsError::InvalidResponse)?;
        match event {
            Event::Start(element) if is_update_item(&namespace, element.local_name().as_ref()) => {
                if update_item_seen || has_suppress_read_receipts(&element)? {
                    return Err(EwsError::InvalidResponse);
                }
                let mut element = element.into_owned();
                element.push_attribute(("SuppressReadReceipts", "true"));
                writer
                    .write_event(Event::Start(element))
                    .map_err(|_| EwsError::InvalidResponse)?;
                update_item_seen = true;
            }
            Event::Empty(element) if is_update_item(&namespace, element.local_name().as_ref()) => {
                return Err(EwsError::InvalidResponse);
            }
            Event::Eof => break,
            event => writer
                .write_event(event.into_owned())
                .map_err(|_| EwsError::InvalidResponse)?,
        }
    }

    if !update_item_seen {
        return Err(EwsError::InvalidResponse);
    }
    Ok(writer.into_inner())
}

fn is_update_item(namespace: &ResolveResult<'_>, local_name: &[u8]) -> bool {
    local_name == b"UpdateItem"
        && matches!(namespace, ResolveResult::Bound(uri) if uri.as_ref() == EWS_MESSAGES_NAMESPACE)
}

fn has_suppress_read_receipts(
    element: &quick_xml::events::BytesStart<'_>,
) -> Result<bool, EwsError> {
    for attribute in element.attributes().with_checks(true) {
        let attribute = attribute.map_err(|_| EwsError::InvalidResponse)?;
        if attribute.key.local_name().as_ref() == b"SuppressReadReceipts" {
            return Ok(true);
        }
    }
    Ok(false)
}
#[cfg(test)]
mod tests;
