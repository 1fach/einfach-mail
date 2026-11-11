use super::{AccountId, EmptyIdentifier, EventId, FolderId, MessageId};

#[test]
fn rejects_empty_provider_identifiers() {
    assert_eq!(MessageId::try_from(String::new()), Err(EmptyIdentifier));
}

#[test]
fn preserves_non_empty_provider_identifiers_exactly() {
    let message_id = MessageId::try_from("AbC/9-Q".to_owned())
        .expect("a non-empty synthetic provider ID is valid");

    assert_eq!(message_id.as_str(), "AbC/9-Q");
}

#[test]
fn debug_output_redacts_every_provider_identifier_kind() {
    let debug_outputs = [
        format!(
            "{:?}",
            AccountId::try_from("account-id-marker".to_owned()).expect("synthetic ID is non-empty")
        ),
        format!(
            "{:?}",
            FolderId::try_from("folder-id-marker".to_owned()).expect("synthetic ID is non-empty")
        ),
        format!(
            "{:?}",
            MessageId::try_from("message-id-marker".to_owned()).expect("synthetic ID is non-empty")
        ),
        format!(
            "{:?}",
            EventId::try_from("event-id-marker".to_owned()).expect("synthetic ID is non-empty")
        ),
    ];

    for (debug, marker) in debug_outputs.iter().zip([
        "account-id-marker",
        "folder-id-marker",
        "message-id-marker",
        "event-id-marker",
    ]) {
        assert!(
            !debug.contains(marker),
            "Debug output included provider ID {marker:?}: {debug}"
        );
        assert!(
            debug.contains("<redacted>"),
            "Debug output omitted the redaction marker: {debug}"
        );
    }
}
