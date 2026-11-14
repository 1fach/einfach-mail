

#[test]
fn mail_reply_body_contains_only_plain_text_message_body() {
    let account = AccountId::try_from("synthetic-account".to_owned()).expect("account ID is valid");
    let folder = FolderReference::new(
        account,
        FolderId::try_from("inbox".to_owned()).expect("folder ID is valid"),
    );
    let source = MailReplySource::new(
        folder,
        MessageId::try_from("source-message".to_owned()).expect("message ID is valid"),
    );
    let draft = MailReplyDraft::new(source, "Synthetic reply\nSecond line".to_owned());
    let serialized = mail_reply_body(&draft).expect("reply request serializes");
    let request: serde_json::Value =
        serde_json::from_slice(&serialized).expect("reply request is valid JSON");

    assert_eq!(request["message"]["body"]["contentType"], "Text");
    assert_eq!(
        request["message"]["body"]["content"],
        "Synthetic reply\nSecond line"
    );
    assert!(request["message"].get("toRecipients").is_none());
    assert!(request["message"].get("subject").is_none());
    assert!(request.get("comment").is_none());
}

#[tokio::test]
async fn message_delta_page_maps_sparse_updates_and_removals() {
    let server = MockServer::start_async().await;
    let delta_link = format!(
        "{}/v1.0/me/mailFolders/inbox/messages/delta?$deltatoken=opaque-final",
        http_base_url(&server)
    );
    let response = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/mailFolders/inbox/messages/delta")
                .query_param(
                    "$select",
                    "id,subject,from,sender,receivedDateTime,isRead,flag,bodyPreview",
                )
                .header("content-type", "application/json");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "value": [
                        {
                            "id": "synthetic-message",
                            "subject": "Updated subject",
                            "from": {"emailAddress": {"address": "sender@example.test", "name": "Sender"}},
                            "sender": null,
                            "receivedDateTime": "2026-10-30T12:00:00Z",
                            "isRead": false
                        },
                        {"id": "synthetic-cleared-message", "subject": null},
                        {"id": "removed-without-reason", "@removed": {}},
                        {"id": "removed-with-reason", "@removed": {"reason": "deleted"}}
                    ],
                    "@odata.deltaLink": delta_link,
                }));
        })
        .await;
    let (api, transport) = synthetic_graph_api(&server);
    let account_id =
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid");
    let folder = FolderReference::new(
        account_id,
        FolderId::try_from("inbox".to_owned()).expect("synthetic folder ID is valid"),
    );

    let MessageDeltaResponse::Page(page) = api
        .message_delta_page(
            &folder,
            MessageDeltaRequest::Initial,
            synthetic_access_token(),
        )
        .await
        .expect("synthetic Graph delta page is mapped")
    else {
        panic!("synthetic delta response is a page");
    };
    let (page, _) = page.into_parts();
    let (mut changes, _) = page.into_parts();
    let MessageDeltaChange::Upsert { id, patch } = changes.remove(0) else {
        panic!("synthetic metadata response is an upsert");
    };
    assert_eq!(id.as_str(), "synthetic-message");
    let mut metadata = MessageMetadata::new(
        Some("Original subject".to_owned()),
        MessageOrigin::new(None, None),
        SystemTime::UNIX_EPOCH,
        ReadState::Read,
        Some(FollowUpFlagStatus::Flagged),
    );
    let expected_received_at = SystemTime::from(
        OffsetDateTime::parse("2026-10-30T12:00:00Z", &Rfc3339)
            .expect("synthetic message time is valid"),
    );
    metadata.apply_patch(patch);
    assert_eq!(metadata.subject(), Some("Updated subject"));
    assert_eq!(
        metadata
            .origin()
            .from_mailbox()
            .map(|mailbox| mailbox.address()),
        Some("sender@example.test")
    );
    assert!(metadata.origin().sender().is_none());
    assert_eq!(metadata.read_state(), ReadState::Unread);
    assert_eq!(metadata.received_at(), &expected_received_at);
    assert_eq!(
        metadata.follow_up_flag_status(),
        Some(FollowUpFlagStatus::Flagged)
    );
    let MessageDeltaChange::Upsert { id, patch } = changes.remove(0) else {
        panic!("synthetic nullable metadata response is an upsert");
    };
    assert_eq!(id.as_str(), "synthetic-cleared-message");
    metadata.apply_patch(patch);
    assert_eq!(metadata.subject(), None);
    assert_eq!(metadata.read_state(), ReadState::Unread);
    assert_eq!(metadata.received_at(), &expected_received_at);
    assert_eq!(
        metadata.follow_up_flag_status(),
        Some(FollowUpFlagStatus::Flagged)
    );
    assert!(matches!(
        changes.remove(0),
        MessageDeltaChange::Removed { id }
            if id.as_str() == "removed-without-reason"
    ));
    assert!(matches!(
        changes.remove(0),
        MessageDeltaChange::Removed { id }
            if id.as_str() == "removed-with-reason"
    ));
    response.assert_async().await;
    drop(transport);
}

#[test]
fn message_delta_rejects_empty_ids_and_invalid_required_metadata() {
    for payload in [
        json!({"id": "", "subject": "Synthetic"}),
        json!({"id": "", "@removed": {}}),
        json!({"id": "synthetic-message", "receivedDateTime": null}),
        json!({"id": "synthetic-message", "isRead": null}),
        json!({
            "id": "synthetic-message",
            "receivedDateTime": "not-a-timestamp",
            "isRead": true
        }),
    ] {
        let wire = serde_json::from_value::<WireMessageChange>(payload)
            .expect("synthetic Graph delta record deserializes");
        assert_eq!(
            MessageDeltaChange::try_from(wire),
            Err(GraphError::InvalidRecord)
        );
    }
}

#[tokio::test]
async fn move_message_sends_one_immutable_id_request_and_checks_destination() {
    let server = MockServer::start_async().await;
    let post = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/v1.0/me/messages/synthetic-message/move")
                .header("prefer", "IdType=\"ImmutableId\"")
                .header("content-type", "application/json")
                .body(r#"{"destinationId":"archive"}"#);
            then.status(201)
                .header("content-type", "application/json")
                .body(r#"{"id":"synthetic-message","parentFolderId":"archive"}"#);
        })
        .await;
    let (graph_api, transport) = synthetic_graph_api(&server);
    let message_id =
        MessageId::try_from("synthetic-message".to_owned()).expect("synthetic message ID is valid");
    let destination = FolderReference::new(
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid"),
        FolderId::try_from("archive".to_owned()).expect("synthetic folder ID is valid"),
    );

    let result = graph_api
        .move_message(&message_id, &destination, synthetic_access_token())
        .await;

    assert_eq!(result, Ok(()));
    post.assert_calls_async(1).await;
    drop(transport);
}

#[tokio::test]
async fn resolves_archive_and_deleted_items_from_well_known_names() {
    let server = MockServer::start_async().await;
    let archive_lookup = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/mailFolders/archive")
                .query_param("$select", "id,displayName,isHidden,unreadItemCount");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"id":"synthetic-archive-folder","displayName":"Archiv","isHidden":false}"#,
                );
        })
        .await;
    let deleted_lookup = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/mailFolders/deleteditems")
                .query_param("$select", "id,displayName,isHidden,unreadItemCount");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"synthetic-deleted-folder","displayName":"Gelöschte Elemente","isHidden":false}"#);
        })
        .await;
    let (graph_api, transport) = synthetic_graph_api(&server);
    let account_id =
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid");

    let archive_folder = graph_api
        .well_known_folder(
            &account_id,
            MailFolderKind::Archive,
            synthetic_access_token(),
        )
        .await
        .expect("localized synthetic Archive folder is resolved");
    let deleted_items_folder = graph_api
        .well_known_folder(
            &account_id,
            MailFolderKind::DeletedItems,
            synthetic_access_token(),
        )
        .await
        .expect("localized synthetic Deleted Items folder is resolved");

    assert_eq!(archive_folder.account_id(), &account_id);
    assert_eq!(
        archive_folder.folder_id().as_str(),
        "synthetic-archive-folder"
    );
    assert_eq!(
        deleted_items_folder.folder_id().as_str(),
        "synthetic-deleted-folder"
    );
    archive_lookup.assert_calls_async(1).await;
    deleted_lookup.assert_calls_async(1).await;
    drop(transport);
}

const FOLDER_KIND_ALIASES: [(&str, MailFolderKind); 6] = [
    ("inbox", MailFolderKind::Inbox),
    ("sentitems", MailFolderKind::SentItems),
    ("drafts", MailFolderKind::Drafts),
    ("archive", MailFolderKind::Archive),
    ("deleteditems", MailFolderKind::DeletedItems),
    ("junkemail", MailFolderKind::JunkEmail),
];

/// Resolves folder kinds against aliases that answer with a folder ID or an error status.
///
/// Returns the resolution and the number of alias requests Graph received.
async fn resolve_folder_kinds_with(
    alias_response: impl Fn(&str) -> Result<String, u16>,
) -> (Result<MailFolderKinds, GraphError>, usize) {
    let server = MockServer::start_async().await;
    let mut lookups = Vec::new();
    for (alias, _) in FOLDER_KIND_ALIASES {
        let response = alias_response(alias);
        let lookup = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path(format!("/v1.0/me/mailFolders/{alias}"))
                    .query_param("$select", "id")
                    .header_missing("prefer");
                match response {
                    Ok(id) => then
                        .status(200)
                        .header("content-type", "application/json")
                        .json_body(json!({"id": id})),
                    Err(status) => then
                        .status(status)
                        .header("content-type", "application/json")
                        .json_body(json!({"error": {"code": "SyntheticFolderError"}})),
                };
            })
            .await;
        lookups.push(lookup);
    }
    let (api, transport) = synthetic_graph_api(&server);
    let kinds = api.folder_kinds(synthetic_access_token()).await;
    let mut requests = 0;
    for lookup in &lookups {
        requests += lookup.calls_async().await;
    }
    drop(transport);
    (kinds, requests)
}

fn resolved_folder(kinds: &MailFolderKinds, kind: MailFolderKind) -> Option<&str> {
    kinds.folder_of(kind).map(FolderId::as_str)
}

#[tokio::test]
async fn folder_kinds_resolve_every_alias_with_one_id_request_each() {
    let (kinds, requests) =
        resolve_folder_kinds_with(|alias| Ok(format!("synthetic-{alias}-folder"))).await;
    let kinds = kinds.expect("every synthetic alias resolves");

    for (alias, kind) in FOLDER_KIND_ALIASES {
        assert_eq!(
            resolved_folder(&kinds, kind),
            Some(format!("synthetic-{alias}-folder").as_str())
        );
    }
    assert_eq!(requests, 6);
}

#[tokio::test]
async fn folder_kinds_leave_out_an_alias_that_answers_not_found() {
    let (kinds, requests) = resolve_folder_kinds_with(|alias| match alias {
        "archive" => Err(404),
        _ => Ok(format!("synthetic-{alias}-folder")),
    })
    .await;
    let kinds = kinds.expect("a missing Archive folder is not an error");

    assert_eq!(resolved_folder(&kinds, MailFolderKind::Archive), None);
    assert_eq!(
        resolved_folder(&kinds, MailFolderKind::JunkEmail),
        Some("synthetic-junkemail-folder")
    );
    assert_eq!(kinds.assignments().count(), 5);
    assert_eq!(requests, 6);
}

#[tokio::test]
async fn folder_kinds_fail_when_an_alias_answers_another_error() {
    for status in [400, 403, 500, 503] {
        let (kinds, _) = resolve_folder_kinds_with(|alias| match alias {
            "drafts" => Err(status),
            _ => Ok(format!("synthetic-{alias}-folder")),
        })
        .await;

        assert!(kinds.is_err(), "status {status} fails the resolution");
    }
}

#[tokio::test]
async fn folder_kinds_keep_the_first_kind_when_two_aliases_name_one_folder() {
    let (kinds, requests) = resolve_folder_kinds_with(|alias| match alias {
        "archive" => Ok("synthetic-inbox-folder".to_owned()),
        _ => Ok(format!("synthetic-{alias}-folder")),
    })
    .await;
    let kinds = kinds.expect("a repeated folder ID is not an error");
    let inbox = FolderId::try_from("synthetic-inbox-folder".to_owned())
        .expect("synthetic folder ID is valid");

    assert_eq!(kinds.kind_of(&inbox), Some(MailFolderKind::Inbox));
    assert_eq!(resolved_folder(&kinds, MailFolderKind::Archive), None);
    assert_eq!(kinds.assignments().count(), 5);
    assert_eq!(requests, 6);
}

#[test]
fn message_move_confirmation_requires_matching_message_and_folder() {
    let message_id =
        MessageId::try_from("synthetic-message".to_owned()).expect("synthetic message ID is valid");
    assert_eq!(
        message_move_body("archive")
            .expect("synthetic move body serializes")
            .as_ref(),
        br#"{"destinationId":"archive"}"#
    );
    assert_eq!(
        confirmed_message_move(
            br#"{"id":"synthetic-message","parentFolderId":"archive"}"#,
            &message_id,
            "archive"
        ),
        Ok(())
    );
    for response in [
        br#"{}"#.as_slice(),
        br#"{"id":"different-message","parentFolderId":"archive"}"#,
        br#"{"id":"synthetic-message","parentFolderId":"inbox"}"#,
    ] {
        assert_eq!(
            confirmed_message_move(response, &message_id, "archive"),
            Err(GraphError::MessageMoveOutcomeUnknown)
        );
    }
}

#[test]
fn read_state_patch_is_minimal_and_requires_server_confirmation() {
    assert_eq!(
        read_state_patch_body(ReadState::Read).as_ref(),
        br#"{"isRead":true}"#
    );
    assert_eq!(
        read_state_patch_body(ReadState::Unread).as_ref(),
        br#"{"isRead":false}"#
    );
    assert_eq!(
        confirmed_read_state(br#"{"isRead":true}"#, ReadState::Read),
        Ok(ReadState::Read)
    );
    assert_eq!(
        confirmed_read_state(br#"{"isRead":false}"#, ReadState::Read),
        Err(GraphError::MessageUpdateOutcomeUnknown)
    );
    assert_eq!(
        confirmed_read_state(br#"{}"#, ReadState::Read),
        Err(GraphError::MessageUpdateOutcomeUnknown)
    );
}

#[test]
fn follow_up_flag_patch_is_minimal_and_requires_matching_confirmation() {
    for (status, payload) in [
        (
            FollowUpFlagStatus::NotFlagged,
            br#"{"flag":{"flagStatus":"notFlagged"}}"#.as_slice(),
        ),
        (
            FollowUpFlagStatus::Flagged,
            br#"{"flag":{"flagStatus":"flagged"}}"#.as_slice(),
        ),
        (
            FollowUpFlagStatus::Complete,
            br#"{"flag":{"flagStatus":"complete"}}"#.as_slice(),
        ),
    ] {
        assert_eq!(follow_up_flag_patch_body(status).as_ref(), payload);
        assert_eq!(confirmed_follow_up_flag_status(payload, status), Ok(status));
    }

    assert_eq!(
        confirmed_follow_up_flag_status(br#"{}"#, FollowUpFlagStatus::Flagged),
        Err(GraphError::MessageUpdateOutcomeUnknown)
    );
    assert_eq!(
        confirmed_follow_up_flag_status(
            br#"{"flag":{"flagStatus":"complete"}}"#,
            FollowUpFlagStatus::Flagged
        ),
        Err(GraphError::MessageUpdateOutcomeUnknown)
    );
    assert!(
        serde_json::from_slice::<WireMessageChange>(
            br#"{"id":"synthetic-message","flag":{"flagStatus":"unknown"}}"#
        )
        .is_err()
    );
}

#[test]
fn delta_preview_updates_keep_an_omitted_preview_and_sanitize_sent_text() {
    let mut metadata = MessageMetadata::new(
        None,
        MessageOrigin::new(None, None),
        SystemTime::UNIX_EPOCH,
        ReadState::Unread,
        None,
    )
    .with_preview(MessagePreview::from_untrusted("Earlier synthetic preview"));
    let expected_cut = "x".repeat(MessagePreview::MAX_CHARS);

    for (body_preview, expected) in [
        (None, Some("Earlier synthetic preview")),
        (
            Some(json!("  Synthetic\r\n\tpreview   text  ")),
            Some("Synthetic preview text"),
        ),
        (Some(json!(null)), None),
        (
            Some(json!("\u{202E}Synthetic\u{2066} reversed\u{200F} text")),
            Some("Synthetic reversed text"),
        ),
        (Some(json!(" \n\t\u{00A0}\u{2028} ")), None),
        (Some(json!("x".repeat(300))), Some(expected_cut.as_str())),
    ] {
        let mut payload = json!({"id": "synthetic-message"});
        if let Some(body_preview) = body_preview {
            payload["bodyPreview"] = body_preview;
        }
        let wire = serde_json::from_value::<WireMessageChange>(payload)
            .expect("synthetic message delta is valid");
        let MessageDeltaChange::Upsert { patch, .. } =
            MessageDeltaChange::try_from(wire).expect("message delta maps to domain values")
        else {
            panic!("synthetic metadata change is an upsert");
        };
        metadata.apply_patch(patch);
        assert_eq!(metadata.preview().map(MessagePreview::as_str), expected);
    }
}

#[test]
fn delta_flag_updates_preserve_omitted_status_and_clear_explicit_null() {
    let mut metadata = MessageMetadata::new(
        None,
        MessageOrigin::new(None, None),
        SystemTime::UNIX_EPOCH,
        ReadState::Unread,
        Some(FollowUpFlagStatus::Flagged),
    );

    for (payload, expected) in [
        (
            br#"{"id":"synthetic-message"}"#.as_slice(),
            Some(FollowUpFlagStatus::Flagged),
        ),
        (
            br#"{"id":"synthetic-message","flag":{"flagStatus":"complete"}}"#.as_slice(),
            Some(FollowUpFlagStatus::Complete),
        ),
        (
            br#"{"id":"synthetic-message","flag":null}"#.as_slice(),
            None,
        ),
        (
            br#"{"id":"synthetic-message","flag":{"flagStatus":null}}"#.as_slice(),
            None,
        ),
    ] {
        let wire = serde_json::from_slice::<WireMessageChange>(payload)
            .expect("synthetic message delta is valid");
        let MessageDeltaChange::Upsert { patch, .. } =
            MessageDeltaChange::try_from(wire).expect("message delta maps to domain values")
        else {
            panic!("synthetic metadata change is an upsert");
        };
        metadata.apply_patch(patch);
        assert_eq!(metadata.follow_up_flag_status(), expected);
    }
}

#[tokio::test]
async fn follow_up_flag_patch_uses_the_message_update_endpoint() {
    let server = MockServer::start_async().await;
    let patch = server
        .mock_async(|when, then| {
            when.method(PATCH)
                .path("/v1.0/me/messages/synthetic-message")
                .header("content-type", "application/json")
                .header("prefer", "IdType=\"ImmutableId\"")
                .body(r#"{"flag":{"flagStatus":"flagged"}}"#);
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"flag":{"flagStatus":"flagged"}}"#);
        })
        .await;
    let (api, _transport) = synthetic_graph_api(&server);
    let message_id =
        MessageId::try_from("synthetic-message".to_owned()).expect("synthetic message ID is valid");

    assert_eq!(
        api.set_message_follow_up_flag_status(
            &message_id,
            FollowUpFlagStatus::Flagged,
            synthetic_access_token(),
        )
        .await,
        Ok(FollowUpFlagStatus::Flagged)
    );

    patch.assert_async().await;
}

#[tokio::test]
async fn read_state_update_sends_minimal_patch_and_returns_exact_confirmation() {
    let server = MockServer::start_async().await;
    let patch = server
        .mock_async(|when, then| {
            when.method(PATCH)
                .path("/v1.0/me/messages/synthetic-message")
                .header("content-type", "application/json")
                .header("prefer", "IdType=\"ImmutableId\"")
                .body(r#"{"isRead":true}"#);
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"isRead":true}"#);
        })
        .await;
    let (graph_api, _transport) = synthetic_graph_api(&server);

    let confirmed = graph_api
        .set_message_read_state(
            &MessageId::try_from("synthetic-message".to_owned())
                .expect("synthetic message ID is valid"),
            ReadState::Read,
            synthetic_access_token(),
        )
        .await
        .expect("Graph confirms the requested read state");

    assert_eq!(confirmed, ReadState::Read);
    patch.assert_async().await;
}

#[tokio::test]
async fn send_mail_posts_one_plain_text_recipient_and_saves_to_sent_items() {
    let server = MockServer::start_async().await;
    let recipient = "ada@example.test"
        .parse::<MailRecipient>()
        .expect("synthetic recipient is valid");
    let subject = "Synthetic subject"
        .parse::<MailSubject>()
        .expect("synthetic subject is valid");
    let draft = MailDraft::new(recipient, subject, "Synthetic body".to_owned());
    let expected_body = json!({
        "message": {
            "subject": "Synthetic subject",
            "body": { "contentType": "Text", "content": "Synthetic body" },
            "toRecipients": [{
                "emailAddress": { "address": "ada@example.test" }
            }]
        },
        "saveToSentItems": true
    });
    let accepted = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/v1.0/me/sendMail")
                .json_body(expected_body);
            then.status(202);
        })
        .await;
    let (api, _transport) = synthetic_graph_api(&server);

    api.send_mail(&draft, synthetic_access_token())
        .await
        .expect("Graph accepts the synthetic plain-text message");

    accepted.assert_async().await;
}