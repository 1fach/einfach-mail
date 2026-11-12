use ews::response::ResponseCode;
use quick_xml::de::from_reader;
use serde::{Deserialize, de::IgnoredAny};

use super::super::validate_calendar_operation_response;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DeleteItemResponseError {
    Rejected(ResponseCode),
    Unknown,
}

pub(super) fn parse_delete_item_response(document: &[u8]) -> Result<(), DeleteItemResponseError> {
    let envelope = parse_delete_response(document)?;
    let message = only_response_message(&envelope)?;
    let code = response_code(message)?;
    match (message.response_class.as_deref(), code) {
        (Some("Success"), ResponseCode::NoError) => Ok(()),
        (Some("Error"), code) if code != ResponseCode::NoError => {
            Err(DeleteItemResponseError::Rejected(code))
        }
        _ => Err(DeleteItemResponseError::Unknown),
    }
}

fn parse_delete_response(document: &[u8]) -> Result<DeleteSoapEnvelope, DeleteItemResponseError> {
    validate_calendar_operation_response(
        document,
        b"DeleteItemResponse",
        b"DeleteItemResponseMessage",
    )
    .map_err(|_| DeleteItemResponseError::Unknown)?;
    from_reader(document).map_err(|_| DeleteItemResponseError::Unknown)
}

fn only_response_message(
    envelope: &DeleteSoapEnvelope,
) -> Result<&DeleteItemResponseMessage, DeleteItemResponseError> {
    let [message] = envelope
        .body
        .delete_item_response
        .response_messages
        .inner
        .as_slice()
    else {
        return Err(DeleteItemResponseError::Unknown);
    };
    Ok(message)
}

fn response_code(
    message: &DeleteItemResponseMessage,
) -> Result<ResponseCode, DeleteItemResponseError> {
    if message.items.is_some() {
        return Err(DeleteItemResponseError::Unknown);
    }
    message
        .response_code
        .ok_or(DeleteItemResponseError::Unknown)
}

#[derive(Deserialize)]
struct DeleteSoapEnvelope {
    #[serde(rename = "Body")]
    body: DeleteSoapBody,
}

#[derive(Deserialize)]
struct DeleteSoapBody {
    #[serde(rename = "DeleteItemResponse")]
    delete_item_response: DeleteSoapResponse,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DeleteSoapResponse {
    response_messages: DeleteResponseMessages,
}

#[derive(Deserialize)]
struct DeleteResponseMessages {
    #[serde(rename = "$value", default)]
    inner: Vec<DeleteItemResponseMessage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
struct DeleteItemResponseMessage {
    #[serde(rename = "@ResponseClass")]
    response_class: Option<String>,
    response_code: Option<ResponseCode>,
    #[serde(rename = "MessageText")]
    _message_text: Option<IgnoredAny>,
    #[serde(rename = "DescriptiveLinkKey")]
    _descriptive_link_key: Option<IgnoredAny>,
    #[serde(rename = "MessageXml")]
    _message_xml: Option<IgnoredAny>,
    items: Option<IgnoredAny>,
}
