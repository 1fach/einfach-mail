#[tokio::test]
async fn ews_read_state_rejects_accounts_without_capability_before_network_access() {
    let settings = ExchangeEwsAccountSettings::new(
        "https://exchange.example.test/EWS/Exchange.asmx".to_owned(),
        "ada@example.test".to_owned(),
        "EXAMPLE\\ada".to_owned(),
    );
    let profile = profile_for(&settings, None).expect("synthetic EWS profile is valid");
    let account_id = profile.id().clone();
    let transport = EwsTransport::new(
        settings.endpoint(),
        settings.username().to_owned(),
        SecretString::from("synthetic-only"),
    )
    .expect("synthetic EWS transport starts without connecting");
    let provider = ExchangeEwsProvider::new(EmptySettingsStore);
    *provider.session.write().await =
        Some(std::sync::Arc::new(EwsSession { profile, transport }));
    let folder = FolderReference::new(
        account_id.clone(),
        FolderId::try_from("synthetic-folder".to_owned())
            .expect("synthetic folder ID is valid"),
    );
    let message_id = MessageId::try_from("synthetic-message-id".to_owned())
        .expect("synthetic message ID is valid");

    assert_eq!(
        provider
            .set_message_read_state(&folder, &message_id, ReadState::Read)
            .await,
        Err(MailOperationError::UnsupportedOperation)
    );
}

#[test]
fn cursor_is_scoped_to_one_folder_and_rejects_unknown_versions() {
    let cursor = encode_cursor("folder-1", Some("server-state".to_owned()))
        .expect("synthetic server cursor is encoded");
    assert_eq!(
        decode_cursor(cursor.as_str())
            .expect("encoded cursor is valid")
            .folder_id,
        "folder-1"
    );
    assert_eq!(
        decode_cursor(r#"{"version":2,"folder_id":"folder-1","sync_state":null}"#),
        Err(MailOperationError::InvalidResponse)
    );
    assert_eq!(
        decode_cursor("x".repeat(300_000).as_str()),
        Err(MailOperationError::ResourceLimitExceeded)
    );
}

#[test]
fn initial_cursor_has_no_sync_state_and_metadata_shape_omits_bodies() {
    let cursor = encode_cursor("folder-1", None).expect("initial cursor is encoded");
    assert_eq!(
        decode_cursor(cursor.as_str())
            .expect("cursor is valid")
            .sync_state,
        None
    );
    let shape = message_metadata_shape();
    assert!(matches!(shape.base_shape, ews::BaseShape::IdOnly));
    assert_eq!(shape.include_mime_content, Some(false));
    assert_eq!(shape.additional_properties.as_ref().map(Vec::len), Some(6));
}

#[test]
fn maps_read_flag_changes_without_requesting_a_full_message() {
    let change = Change::ReadFlagChange {
        item_id: ews::ItemId {
            id: "message-1".to_owned(),
            change_key: None,
        },
        is_read: true,
    };
    let mapped = map_item_change(change)
        .expect("synthetic read flag change maps")
        .expect("read flag change is retained");
    assert!(matches!(mapped, MessageDeltaChange::Upsert { .. }));
}

#[test]
fn maps_a_synthetic_message_page_and_preserves_its_continuation_cursor() {
    let responses = parse_soap_response::<SyncFolderItems>(SYNC_FOLDER_ITEMS_PAGE)
        .expect("synthetic EWS page fixture parses");
    let response = map_sync_responses(
        responses.messages,
        "synthetic-folder",
        SYNC_FOLDER_ITEMS_PAGE.len(),
    )
    .expect("synthetic EWS page maps");
    let MessageDeltaResponse::Page(response) = response else {
        panic!("successful EWS page maps to a message page");
    };
    let (page, response_bytes) = response.into_parts();
    assert_eq!(response_bytes, SYNC_FOLDER_ITEMS_PAGE.len());
    let (mut changes, progress) = page.into_parts();
    let MessageDeltaProgress::Next(link) = progress else {
        panic!("fixture indicates another page remains");
    };
    let cursor = decode_cursor(link.as_str()).expect("continuation cursor decodes");
    assert_eq!(cursor.folder_id, "synthetic-folder");
    assert_eq!(cursor.sync_state.as_deref(), Some("synthetic-page-cursor"));

    let Some(MessageDeltaChange::Upsert { id, patch }) = changes.pop() else {
        panic!("fixture includes one message upsert");
    };
    assert_eq!(id.as_str(), "synthetic-message-id");
    let account_id = AccountId::try_from("synthetic-account".to_owned())
        .expect("synthetic account ID is valid");
    let folder_id = FolderId::try_from("synthetic-folder".to_owned())
        .expect("synthetic folder ID is valid");
    let message = patch
        .into_complete_message(FolderReference::new(account_id, folder_id), id)
        .expect("fixture includes complete metadata");
    assert_eq!(message.metadata().subject(), Some("Synthetic EWS fixture"));
    assert_eq!(message.metadata().read_state(), ReadState::Unread);
    assert_eq!(
        message
            .metadata()
            .origin()
            .from_mailbox()
            .map(|mailbox| mailbox.address()),
        Some("ada@example.test")
    );
}

#[test]
fn maps_a_synthetic_expired_cursor_response_to_a_reset() {
    let responses = parse_soap_response::<SyncFolderItems>(SYNC_STATE_EXPIRED)
        .expect("expired-state EWS fixture parses");
    let response = map_sync_responses(
        responses.messages,
        "synthetic-folder",
        SYNC_STATE_EXPIRED.len(),
    )
    .expect("expired EWS cursor requests a replacement snapshot");
    let MessageDeltaResponse::Reset(link) = response else {
        panic!("expired EWS cursor maps to a reset link");
    };
    let cursor = decode_cursor(link.as_str()).expect("reset cursor decodes");
    assert_eq!(cursor.folder_id, "synthetic-folder");
    assert_eq!(cursor.sync_state, None);
}

#[test]
fn maps_access_denied_codes_and_preserves_other_mail_response_errors() {
    for (response_code, expected_error) in [
        ("ErrorAccessDenied", MailOperationError::AccessDenied),
        (
            "ErrorImpersonateUserDenied",
            MailOperationError::AccessDenied,
        ),
        (
            "ErrorInternalServerError",
            MailOperationError::InvalidResponse,
        ),
    ] {
        let document = String::from_utf8_lossy(SYNC_STATE_EXPIRED)
            .replace("ErrorInvalidSyncStateData", response_code);
        let response = parse_soap_response::<SyncFolderItems>(document.as_bytes())
            .expect("synthetic EWS error response parses");

        assert_eq!(
            map_sync_responses(response.messages, "synthetic-folder", document.len()),
            Err(expected_error)
        );
    }
}
