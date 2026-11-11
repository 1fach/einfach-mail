use einfach_mail_domain::{MessageId, ReadState};
use ews::{
    ItemId, Items, Message, RealItem, ResponseClass,
    response::{ResponseCode, ResponseError},
    update_item::UpdateItemResponseMessage,
};

use crate::transport::EwsSoapResponse;

use super::{
    CurrentEwsItemVersion, GetItemError, ParsedUpdateItemResponse, UpdateConfirmationError,
    confirm_update, current_item_version, parse_get_item_response, parse_update_item_response,
    serialize_update_item_request,
};

const UPDATE_ITEM_SUCCESS: &[u8] = include_bytes!("../../..//fixtures/update-item-success.xml");
const UPDATE_ITEM_CONFLICT: &[u8] = include_bytes!("../../..//fixtures/update-item-conflict.xml");
const UPDATE_ITEM_WARNING: &[u8] = include_bytes!("../../..//fixtures/update-item-warning.xml");
const GET_ITEM_VERSIONED: &[u8] = include_bytes!("../../..//fixtures/get-item-versioned.xml");

fn synthetic_message_id() -> MessageId {
    MessageId::try_from("synthetic-message-id".to_owned()).expect("synthetic message ID is valid")
}

fn synthetic_current_item_version() -> CurrentEwsItemVersion {
    CurrentEwsItemVersion::from_message(
        &Message {
            item_id: Some(ItemId {
                id: "synthetic-message-id".to_owned(),
                change_key: Some("synthetic-change-key".to_owned()),
            }),
            ..Message::default()
        },
        &synthetic_message_id(),
    )
    .expect("synthetic item version is valid")
}

fn response_with_message(
    message: Message,
    response_code: ResponseCode,
) -> ParsedUpdateItemResponse {
    ParsedUpdateItemResponse {
        response: EwsSoapResponse {
            messages: vec![ResponseClass::Success(UpdateItemResponseMessage {
                items: Items {
                    inner: vec![RealItem::Message(message)],
                },
            })],
            server_version: None,
        },
        response_codes: vec![Some(response_code)],
    }
}

#[test]
fn serializes_only_the_requested_read_state_with_receipts_suppressed() {
    for (requested, serialized) in [
        (ReadState::Read, "<t:IsRead>true</t:IsRead>"),
        (ReadState::Unread, "<t:IsRead>false</t:IsRead>"),
    ] {
        let document = String::from_utf8(
            serialize_update_item_request(&synthetic_current_item_version(), requested)
                .expect("synthetic UpdateItem serializes"),
        )
        .expect("serialized EWS request is UTF-8");

        assert!(document.contains("SuppressReadReceipts=\"true\""));
        assert!(document.contains("MessageDisposition=\"SaveOnly\""));
        assert!(document.contains("ConflictResolution=\"NeverOverwrite\""));
        assert!(document.contains("Id=\"synthetic-message-id\""));
        assert!(document.contains("ChangeKey=\"synthetic-change-key\""));
        assert!(document.contains(serialized));
        assert_eq!(document.matches("SuppressReadReceipts").count(), 1);
    }
}

#[test]
fn rejects_a_change_key_for_a_different_or_unversioned_item() {
    let message = Message {
        item_id: Some(ItemId {
            id: "another-message-id".to_owned(),
            change_key: Some("synthetic-change-key".to_owned()),
        }),
        ..Message::default()
    };
    assert!(CurrentEwsItemVersion::from_message(&message, &synthetic_message_id()).is_err());

    let message = Message {
        item_id: Some(ItemId {
            id: "synthetic-message-id".to_owned(),
            change_key: Some(String::new()),
        }),
        ..Message::default()
    };
    assert!(CurrentEwsItemVersion::from_message(&message, &synthetic_message_id()).is_err());
}

#[test]
fn requires_an_explicit_no_error_code_for_a_fresh_get_item_version() {
    let parsed =
        parse_get_item_response(GET_ITEM_VERSIONED).expect("synthetic GetItem response parses");
    let (server_version, current) =
        current_item_version(parsed, &synthetic_message_id()).expect("GetItem is successful");
    assert_eq!(
        server_version.and_then(|version| version.major_version),
        Some("15".to_owned())
    );
    assert_eq!(current.id, "synthetic-message-id");
    assert_eq!(current.change_key, "synthetic-change-key");

    let contradictory_response = String::from_utf8(GET_ITEM_VERSIONED.to_vec())
        .expect("synthetic SOAP fixture is UTF-8")
        .replace(
            "<m:ResponseCode>NoError</m:ResponseCode>",
            "<m:ResponseCode>ErrorAccessDenied</m:ResponseCode>",
        );
    let parsed = parse_get_item_response(contradictory_response.as_bytes())
        .expect("contradictory response remains structurally valid");
    assert!(matches!(
        current_item_version(parsed, &synthetic_message_id()),
        Err(GetItemError::InvalidResponse)
    ));
}

#[test]
fn confirms_only_one_matching_item_with_a_new_or_current_change_key() {
    let response = |change_key: &str| {
        response_with_message(
            Message {
                item_id: Some(ItemId {
                    id: "synthetic-message-id".to_owned(),
                    change_key: Some(change_key.to_owned()),
                }),
                ..Message::default()
            },
            ResponseCode::NoError,
        )
    };

    assert_eq!(
        confirm_update(
            response("synthetic-change-key"),
            &synthetic_message_id(),
            ReadState::Read
        ),
        Ok(ReadState::Read)
    );
    assert_eq!(
        confirm_update(
            response("next-change-key"),
            &synthetic_message_id(),
            ReadState::Unread
        ),
        Ok(ReadState::Unread)
    );
}

#[test]
fn parses_and_confirms_a_synthetic_update_item_soap_response() {
    let response = parse_update_item_response(UPDATE_ITEM_SUCCESS)
        .expect("synthetic UpdateItem response parses");

    assert_eq!(
        confirm_update(response, &synthetic_message_id(), ReadState::Read),
        Ok(ReadState::Read)
    );
}

#[test]
fn rejects_mismatched_success_and_ambiguous_update_responses() {
    let wrong_id = response_with_message(
        Message {
            item_id: Some(ItemId {
                id: "another-message-id".to_owned(),
                change_key: Some("synthetic-change-key".to_owned()),
            }),
            ..Message::default()
        },
        ResponseCode::NoError,
    );
    assert_eq!(
        confirm_update(wrong_id, &synthetic_message_id(), ReadState::Read),
        Err(UpdateConfirmationError::Unknown)
    );
    assert_eq!(
        confirm_update(
            ParsedUpdateItemResponse {
                response: EwsSoapResponse {
                    messages: Vec::new(),
                    server_version: None,
                },
                response_codes: Vec::new(),
            },
            &synthetic_message_id(),
            ReadState::Read
        ),
        Err(UpdateConfirmationError::Unknown)
    );
}

#[test]
fn requires_no_error_response_code_for_success() {
    let success = response_with_message(
        Message {
            item_id: Some(ItemId {
                id: "synthetic-message-id".to_owned(),
                change_key: Some("synthetic-change-key".to_owned()),
            }),
            ..Message::default()
        },
        ResponseCode::ErrorInvalidChangeKey,
    );
    assert_eq!(
        confirm_update(success, &synthetic_message_id(), ReadState::Read),
        Err(UpdateConfirmationError::Unknown)
    );

    let rejected = ParsedUpdateItemResponse {
        response: EwsSoapResponse {
            messages: vec![ResponseClass::Error(ResponseError {
                message_text: "synthetic rejection".to_owned(),
                response_code: ResponseCode::ErrorInvalidChangeKey,
                message_xml: None,
            })],
            server_version: None,
        },
        response_codes: vec![Some(ResponseCode::ErrorInvalidChangeKey)],
    };
    assert_eq!(
        confirm_update(rejected, &synthetic_message_id(), ReadState::Read),
        Err(UpdateConfirmationError::Rejected(
            ResponseCode::ErrorInvalidChangeKey
        ))
    );
}

#[test]
fn parses_an_explicit_conflict_as_rejected_and_a_warning_as_unknown() {
    let conflict = parse_update_item_response(UPDATE_ITEM_CONFLICT)
        .expect("synthetic conflict response parses");
    assert_eq!(
        confirm_update(conflict, &synthetic_message_id(), ReadState::Read),
        Err(UpdateConfirmationError::Rejected(
            ResponseCode::ErrorInvalidChangeKey
        ))
    );

    let warning =
        parse_update_item_response(UPDATE_ITEM_WARNING).expect("synthetic warning response parses");
    assert_eq!(
        confirm_update(warning, &synthetic_message_id(), ReadState::Read),
        Err(UpdateConfirmationError::Unknown)
    );
}
