use einfach_mail_application::CalendarOperationError;
use einfach_mail_domain::{CalendarEventVersion, EventId};
use ews::response::ResponseCode;
use quick_xml::de::from_reader;
use serde::{Deserialize, de::IgnoredAny};

use super::super::{map_access_denied_response_code, validate_calendar_operation_response};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum UpdateItemResponseError {
    Rejected(ResponseCode),
    Unknown,
}

pub(super) fn map_update_response_error(error: UpdateItemResponseError) -> CalendarOperationError {
    match error {
        UpdateItemResponseError::Rejected(code) => match code {
            ResponseCode::ErrorIrresolvableConflict
            | ResponseCode::ErrorItemNotFound
            | ResponseCode::ErrorStaleObject => CalendarOperationError::EventChanged,
            _ => map_access_denied_response_code(code)
                .map_or(CalendarOperationError::EventUpdateRejected, Into::into),
        },
        UpdateItemResponseError::Unknown => CalendarOperationError::UpdateOutcomeUnknown,
    }
}

pub(super) fn parse_update_item_response(
    document: &[u8],
    expected_id: &EventId,
) -> Result<CalendarEventVersion, UpdateItemResponseError> {
    let envelope = parse_update_response(document)?;
    let message = only_response_message(&envelope)?;
    match (message.response_class.as_deref(), message.response_code) {
        (Some("Success"), Some(ResponseCode::NoError)) => parse_updated_item(message, expected_id),
        (Some("Error"), Some(code)) if code != ResponseCode::NoError => {
            reject_without_items(message, code)
        }
        _ => Err(UpdateItemResponseError::Unknown),
    }
}

fn only_response_message(
    envelope: &UpdateSoapEnvelope,
) -> Result<&UpdateResponseMessage, UpdateItemResponseError> {
    let [message] = envelope
        .body
        .update_item_response
        .response_messages
        .inner
        .as_slice()
    else {
        return Err(UpdateItemResponseError::Unknown);
    };
    Ok(message)
}

fn parse_update_response(document: &[u8]) -> Result<UpdateSoapEnvelope, UpdateItemResponseError> {
    validate_calendar_operation_response(
        document,
        b"UpdateItemResponse",
        b"UpdateItemResponseMessage",
    )
    .map_err(|_| UpdateItemResponseError::Unknown)?;
    from_reader(document).map_err(|_| UpdateItemResponseError::Unknown)
}

fn parse_updated_item(
    message: &UpdateResponseMessage,
    expected_id: &EventId,
) -> Result<CalendarEventVersion, UpdateItemResponseError> {
    let item_id = response_item_id(message)?;
    if item_id.id.as_deref() != Some(expected_id.as_str()) {
        return Err(UpdateItemResponseError::Unknown);
    }
    let change_key = item_id
        .change_key
        .as_deref()
        .filter(|value| valid_nonempty_text(value))
        .ok_or(UpdateItemResponseError::Unknown)?;
    CalendarEventVersion::try_from(change_key.to_owned())
        .map_err(|_| UpdateItemResponseError::Unknown)
}

fn response_item_id(
    message: &UpdateResponseMessage,
) -> Result<&UpdateResponseItemId, UpdateItemResponseError> {
    let [UpdateResponseItem::CalendarItem(item)] = message
        .items
        .as_ref()
        .map(|items| items.inner.as_slice())
        .unwrap_or_default()
    else {
        return Err(UpdateItemResponseError::Unknown);
    };
    item.item_id
        .as_ref()
        .ok_or(UpdateItemResponseError::Unknown)
}

fn reject_without_items(
    message: &UpdateResponseMessage,
    code: ResponseCode,
) -> Result<CalendarEventVersion, UpdateItemResponseError> {
    if message
        .items
        .as_ref()
        .is_some_and(|items| !items.inner.is_empty())
    {
        return Err(UpdateItemResponseError::Unknown);
    }
    Err(UpdateItemResponseError::Rejected(code))
}

fn valid_nonempty_text(value: &str) -> bool {
    !value.trim().is_empty() && !value.chars().any(char::is_control)
}

#[derive(Deserialize)]
struct UpdateSoapEnvelope {
    #[serde(rename = "Body")]
    body: UpdateSoapBody,
}

#[derive(Deserialize)]
struct UpdateSoapBody {
    #[serde(rename = "UpdateItemResponse")]
    update_item_response: UpdateSoapResponse,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UpdateSoapResponse {
    response_messages: UpdateResponseMessages,
}

#[derive(Deserialize)]
struct UpdateResponseMessages {
    #[serde(rename = "$value", default)]
    inner: Vec<UpdateResponseMessage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UpdateResponseMessage {
    #[serde(rename = "@ResponseClass")]
    response_class: Option<String>,
    response_code: Option<ResponseCode>,
    items: Option<UpdateResponseItems>,
}

#[derive(Deserialize)]
struct UpdateResponseItems {
    #[serde(rename = "$value", default)]
    inner: Vec<UpdateResponseItem>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
enum UpdateResponseItem {
    CalendarItem(UpdateResponseCalendarItem),
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
struct UpdateResponseCalendarItem {
    item_id: Option<UpdateResponseItemId>,
}

#[derive(Deserialize)]
struct UpdateResponseItemId {
    #[serde(rename = "@Id")]
    id: Option<String>,
    #[serde(rename = "@ChangeKey")]
    change_key: Option<String>,
}
