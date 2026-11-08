use std::{sync::Arc, time::SystemTime};

use httpmock::{
    Method::{DELETE, GET, PATCH, POST},
    MockServer,
};
use huskarl::{
    core::secrets::SecretString,
    token::{AccessToken, BearerAccessToken},
};
use serde_json::json;
use time::{
    Date, Month, OffsetDateTime, PrimitiveDateTime, Time, format_description::well_known::Rfc3339,
};
use url::Url;

use einfach_mail_application::{
    CalendarDeltaRequest, CalendarDeltaResponse, MessageDeltaRequest, MessageDeltaResponse,
    WellKnownMailFolder,
};
use einfach_mail_domain::{
    AccountId, CalendarDeltaChange, CalendarDeltaLink, CalendarDeltaProgress,
    CalendarEventCreation, CalendarEventDraft, CalendarEventUpdate, CalendarEventUpdateDraft,
    CalendarOccurrence, CalendarOccurrenceTime, CalendarTimeZoneId, CalendarViewRange,
    CalendarViewReference, EditableCalendarEvent, FolderId, FolderReference, FollowUpFlagStatus,
    MailDraft, MailRecipient, MailReplyDraft, MailReplySource, MailSubject, MessageDeltaChange,
    MessageId, MessageMetadata, MessageOrigin, ReadState,
};

use super::{
    GraphApi,
    calendar::{WireCalendarEvent, calendar_event_update_body, create_calendar_event_body},
    mail::{
        WireFolder, WireMessageChange, confirmed_follow_up_flag_status, confirmed_message_move,
        confirmed_read_state, follow_up_flag_patch_body, mail_reply_body, message_move_body,
        read_state_patch_body,
    },
    pagination::{MAX_PAGINATION_PAGES, MAX_PAGINATION_RESPONSE_BYTES, PaginationBudget},
};
use crate::{
    error::GraphError,
    graph_url::{CALENDAR_EVENT_EDIT_SELECT, GraphAuthority},
    transport::{GraphTransport, GraphTransportHandle},
};

fn synthetic_access_token() -> Arc<AccessToken> {
    Arc::new(AccessToken::Bearer(BearerAccessToken::new(
        SecretString::new("synthetic-calendar-access-token"),
        SystemTime::now(),
        Some(std::time::Duration::from_secs(3600)),
    )))
}

fn synthetic_view() -> CalendarViewReference {
    let account =
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid");
    let start_date = Date::from_calendar_date(2030, Month::January, 7)
        .expect("synthetic week start date is valid");
    let end_date = Date::from_calendar_date(2030, Month::January, 14)
        .expect("synthetic week end date is valid");
    let range = CalendarViewRange::new(
        start_date.midnight().assume_utc(),
        end_date.midnight().assume_utc(),
        start_date,
        end_date,
    )
    .expect("synthetic calendar range is valid");
    CalendarViewReference::new(account, range)
}

fn synthetic_utc_datetime(date_time_text: &str) -> OffsetDateTime {
    OffsetDateTime::parse(date_time_text, &Rfc3339).expect("synthetic UTC date-time is valid")
}

fn recurring_graph_event_json(id: &str, start: &str, end: &str) -> serde_json::Value {
    json!({
        "id": id,
        "type": "occurrence",
        "seriesMasterId": "synthetic-weekly-series",
        "start": {"dateTime": start, "timeZone": "UTC"},
        "end": {"dateTime": end, "timeZone": "UTC"},
        "isAllDay": false,
        "originalStartTimeZone": "W. Europe Standard Time",
        "originalEndTimeZone": "W. Europe Standard Time"
    })
}

fn assert_dst_occurrence(occurrence: &CalendarOccurrence, event_id: &str, start: &str, end: &str) {
    assert_eq!(
        occurrence.id().as_str(),
        event_id,
        "the Graph occurrence keeps its event ID"
    );
    assert_eq!(
        occurrence.time(),
        &CalendarOccurrenceTime::Timed {
            start: synthetic_utc_datetime(start),
            end: synthetic_utc_datetime(end)
        },
        "the Graph occurrence keeps its returned UTC instants"
    );
    assert_eq!(
        occurrence.original_start_time_zone(),
        Some("W. Europe Standard Time"),
        "the source start-zone ID remains opaque metadata"
    );
    assert_eq!(
        occurrence.original_end_time_zone(),
        Some("W. Europe Standard Time"),
        "the source end-zone ID remains opaque metadata"
    );
}

fn occurrence_from_graph_change(
    change: CalendarDeltaChange,
    view: &CalendarViewReference,
) -> CalendarOccurrence {
    match change {
        CalendarDeltaChange::Upsert { id, patch } => patch
            .into_new(view.clone(), id)
            .expect("synthetic Graph occurrence is complete"),
        CalendarDeltaChange::Removed { .. } => {
            panic!("synthetic Graph delta returns only occurrences")
        }
    }
}

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
async fn folder_listing_follows_pages_and_traverses_children() {
    let server = MockServer::start_async().await;
    let page_two_link = format!(
        "{}/v1.0/me/mailFolders?$skiptoken=opaque-root-page",
        server.base_url()
    );
    let root_page_one = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/mailFolders")
                .query_param_missing("$skiptoken");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "value": [{"id": "root-inbox", "displayName": "Inbox"}],
                    "@odata.nextLink": page_two_link,
                }));
        })
        .await;
    let root_page_two = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/mailFolders")
                .query_param("$skiptoken", "opaque-root-page");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "value": [{"id": "root-archive", "displayName": "Archive"}]
                }));
        })
        .await;
    let inbox_children = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/mailFolders/root-inbox/childFolders");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "value": [{"id": "inbox-projects", "displayName": "Projects"}]
                }));
        })
        .await;
    let archive_children = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/mailFolders/root-archive/childFolders");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({"value": []}));
        })
        .await;
    let projects_children = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/mailFolders/inbox-projects/childFolders");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({"value": []}));
        })
        .await;
    let (api, transport) = synthetic_graph_api(&server);
    let account_id =
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid");

    let folders = api
        .list_folders(&account_id, synthetic_access_token())
        .await
        .expect("all synthetic root and child folders are listed");

    assert_eq!(
        folders
            .iter()
            .map(|folder| folder.reference().folder_id().as_str())
            .collect::<Vec<_>>(),
        ["root-inbox", "root-archive", "inbox-projects"]
    );
    assert_eq!(
        folders[2]
            .parent()
            .expect("synthetic Projects folder has its parent")
            .folder_id()
            .as_str(),
        "root-inbox"
    );
    root_page_one.assert_async().await;
    root_page_two.assert_async().await;
    inbox_children.assert_async().await;
    archive_children.assert_async().await;
    projects_children.assert_async().await;
    drop(transport);
}

#[tokio::test]
async fn folder_listing_rejects_a_repeated_page_link_before_refetching() {
    let server = MockServer::start_async().await;
    let (api, transport) = synthetic_graph_api(&server);
    let first_page = api
        .authority
        .root_folders_path()
        .expect("synthetic root folder URL is valid");
    let repeated_page = first_page.as_str().to_owned();
    let root_page = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/mailFolders")
                .query_param_missing("$skiptoken");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "value": [],
                    "@odata.nextLink": repeated_page,
                }));
        })
        .await;
    let account_id =
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid");

    assert_eq!(
        api.list_folders(&account_id, synthetic_access_token())
            .await,
        Err(GraphError::UnsafeLink)
    );
    root_page.assert_calls_async(1).await;
    drop(transport);
}

#[tokio::test]
async fn message_delta_page_maps_sparse_updates_and_removals() {
    let server = MockServer::start_async().await;
    let delta_link = format!(
        "{}/v1.0/me/mailFolders/inbox/messages/delta?$deltatoken=opaque-final",
        server.base_url()
    );
    let response = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/mailFolders/inbox/messages/delta")
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

#[tokio::test]
async fn folder_listing_rejects_duplicate_ids_in_the_folder_tree() {
    let server = MockServer::start_async().await;
    let roots = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/mailFolders")
                .query_param_missing("$skiptoken");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "value": [
                        {"id": "root-inbox", "displayName": "Inbox"},
                        {"id": "root-archive", "displayName": "Archive"}
                    ]
                }));
        })
        .await;
    let duplicate_child = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/mailFolders/root-inbox/childFolders");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "value": [{"id": "root-archive", "displayName": "Duplicate"}]
                }));
        })
        .await;
    let unvisited_children = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/mailFolders/root-archive/childFolders");
            then.status(200).json_body(json!({"value": []}));
        })
        .await;
    let (api, transport) = synthetic_graph_api(&server);
    let account_id =
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid");

    assert_eq!(
        api.list_folders(&account_id, synthetic_access_token())
            .await,
        Err(GraphError::InvalidRecord)
    );
    roots.assert_async().await;
    duplicate_child.assert_async().await;
    unvisited_children.assert_calls_async(0).await;
    drop(transport);
}

#[tokio::test]
async fn profile_read_does_not_follow_a_graph_redirect() {
    let server = MockServer::start_async().await;
    let redirect = server
        .mock_async(|when, then| {
            when.method(GET).path("/v1.0/me");
            then.status(302).header("Location", "/redirected");
        })
        .await;
    let redirect_target = server
        .mock_async(|when, then| {
            when.method(GET).path("/redirected");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"synthetic-account"}"#);
        })
        .await;
    let (api, transport) = synthetic_graph_api(&server);
    let profile_endpoint =
        Url::parse(&server.url("/v1.0/me")).expect("synthetic Graph profile endpoint is valid");

    assert_eq!(
        api.load_profile(&profile_endpoint, synthetic_access_token())
            .await,
        Err(GraphError::Transport)
    );
    redirect.assert_calls_async(1).await;
    redirect_target.assert_calls_async(0).await;
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

fn editable_event_json(version: &str, subject: &str, start: &str, end: &str) -> serde_json::Value {
    json!({
        "id": "synthetic-event",
        "subject": subject,
        "location": {"displayName": "Room A"},
        "start": {"dateTime": start, "timeZone": "UTC"},
        "end": {"dateTime": end, "timeZone": "UTC"},
        "isAllDay": false,
        "originalStartTimeZone": "UTC",
        "originalEndTimeZone": "UTC",
        "type": "singleInstance",
        "isOrganizer": true,
        "attendees": [],
        "isOnlineMeeting": false,
        "changeKey": version,
        "@odata.etag": "W/\"synthetic-etag-1\""
    })
}

fn editable_event(version: &str) -> EditableCalendarEvent {
    serde_json::from_value::<WireCalendarEvent>(editable_event_json(
        version,
        "Planning",
        "2030-01-08T09:00:00.0000000",
        "2030-01-08T10:00:00.0000000",
    ))
    .expect("synthetic event metadata deserializes")
    .into_editable(synthetic_view())
    .expect("synthetic single-instance appointment is editable")
}

fn utc_time(day: u8, hour: u8) -> OffsetDateTime {
    Date::from_calendar_date(2030, Month::January, day)
        .expect("synthetic event date is valid")
        .with_hms(hour, 0, 0)
        .expect("synthetic event time is valid")
        .assume_utc()
}

fn synthetic_graph_api(server: &MockServer) -> (GraphApi, GraphTransport) {
    let endpoint =
        Url::parse(&server.url("/v1.0/me")).expect("synthetic Graph profile endpoint is valid");
    let authority = GraphAuthority::from_profile_endpoint(&endpoint)
        .expect("synthetic Graph authority is valid");
    let transport =
        GraphTransport::start_for_fixtures().expect("synthetic Graph transport worker starts");
    let handle: GraphTransportHandle = transport.handle();
    (GraphApi::new(authority, handle), transport)
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
                .query_param("$select", "id,displayName,isHidden");
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
                .query_param("$select", "id,displayName,isHidden");
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
            WellKnownMailFolder::Archive,
            synthetic_access_token(),
        )
        .await
        .expect("localized synthetic Archive folder is resolved");
    let deleted_items_folder = graph_api
        .well_known_folder(
            &account_id,
            WellKnownMailFolder::DeletedItems,
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

#[test]
fn create_event_request_contains_only_timed_primary_calendar_fields() {
    let view = synthetic_view();
    let date =
        Date::from_calendar_date(2030, Month::January, 8).expect("synthetic event date is valid");
    let local_start = PrimitiveDateTime::new(
        date,
        Time::from_hms(9, 30, 0).expect("synthetic start time is valid"),
    );
    let local_end = PrimitiveDateTime::new(
        date,
        Time::from_hms(10, 0, 0).expect("synthetic end time is valid"),
    );
    let draft = CalendarEventDraft::timed(
        "Synthetic planning meeting".to_owned(),
        local_start,
        local_end,
        CalendarTimeZoneId::try_from("Etc/UTC".to_owned()).expect("synthetic time zone is valid"),
        date.with_hms(9, 30, 0)
            .expect("synthetic UTC start is valid")
            .assume_utc(),
        date.with_hms(10, 0, 0)
            .expect("synthetic UTC end is valid")
            .assume_utc(),
    )
    .expect("synthetic event draft is valid");
    let creation =
        CalendarEventCreation::try_new(view, draft).expect("synthetic event fits its week");

    let body = create_calendar_event_body(&creation).expect("synthetic event body serializes");
    let body: serde_json::Value =
        serde_json::from_slice(&body).expect("synthetic event body is JSON");
    let object = body
        .as_object()
        .expect("synthetic event body is a JSON object");
    let mut keys = object.keys().map(String::as_str).collect::<Vec<_>>();
    keys.sort_unstable();

    assert_eq!(keys, ["end", "start", "subject", "transactionId"]);
    assert_eq!(body["subject"], "Synthetic planning meeting");
    assert_eq!(body["start"]["dateTime"], "2030-01-08T09:30:00");
    assert_eq!(body["start"]["timeZone"], "UTC");
    assert_eq!(body["end"]["dateTime"], "2030-01-08T10:00:00");
    assert_eq!(body["end"]["timeZone"], "UTC");
    assert_eq!(body["transactionId"], creation.id().as_uuid().to_string());
}

#[test]
fn event_update_body_contains_only_changed_fields() {
    let subject_update = CalendarEventUpdate::new(
        editable_event("synthetic-version-1"),
        CalendarEventUpdateDraft::timed(
            "Updated planning".to_owned(),
            utc_time(8, 9),
            utc_time(8, 10),
        )
        .expect("synthetic subject update is valid"),
    )
    .expect("synthetic subject changed");
    let subject_body: serde_json::Value = serde_json::from_slice(
        &calendar_event_update_body(&subject_update).expect("synthetic subject update serializes"),
    )
    .expect("subject update is JSON");
    assert_eq!(subject_body, json!({"subject": "Updated planning"}));

    let time_update = CalendarEventUpdate::new(
        editable_event("synthetic-version-1"),
        CalendarEventUpdateDraft::timed("Planning".to_owned(), utc_time(8, 11), utc_time(8, 12))
            .expect("synthetic time update is valid"),
    )
    .expect("synthetic time changed");
    let time_body: serde_json::Value = serde_json::from_slice(
        &calendar_event_update_body(&time_update).expect("synthetic time update serializes"),
    )
    .expect("time update is JSON");
    assert_eq!(
        time_body,
        json!({
            "start": {"dateTime": "2030-01-08T11:00:00", "timeZone": "UTC"},
            "end": {"dateTime": "2030-01-08T12:00:00", "timeZone": "UTC"}
        })
    );
}

#[test]
fn event_edit_boundary_rejects_meetings_recurring_events_and_unknown_metadata() {
    let valid_event_json = editable_event_json(
        "synthetic-version-1",
        "Planning",
        "2030-01-08T09:00:00.0000000",
        "2030-01-08T10:00:00.0000000",
    );
    for event_json in [
        {
            let mut event_json = valid_event_json.clone();
            event_json["type"] = json!("occurrence");
            event_json
        },
        {
            let mut event_json = valid_event_json.clone();
            event_json["isOrganizer"] = json!(false);
            event_json
        },
        {
            let mut event_json = valid_event_json.clone();
            event_json["attendees"] = json!([{}]);
            event_json
        },
        {
            let mut event_json = valid_event_json.clone();
            event_json["isOnlineMeeting"] = json!(true);
            event_json
        },
        {
            let mut event_json = valid_event_json.clone();
            event_json["isAllDay"] = json!(true);
            event_json
        },
        {
            let mut event_json = valid_event_json.clone();
            event_json["subject"] = serde_json::Value::Null;
            event_json
        },
        {
            let mut event_json = valid_event_json.clone();
            event_json["subject"] = json!("");
            event_json
        },
        {
            let mut event_json = valid_event_json.clone();
            event_json["subject"] = json!("Planning\tmeeting");
            event_json
        },
        {
            let mut event_json = valid_event_json.clone();
            event_json
                .as_object_mut()
                .expect("synthetic event is an object")
                .remove("changeKey");
            event_json
        },
    ] {
        let wire = serde_json::from_value::<WireCalendarEvent>(event_json)
            .expect("synthetic event metadata deserializes");
        assert_eq!(
            wire.into_editable(synthetic_view()),
            Err(GraphError::CalendarEventNotEditable)
        );
    }
}

#[tokio::test]
async fn event_update_rechecks_version_and_patches_only_requested_fields() {
    let server = MockServer::start_async().await;
    let view = synthetic_view();
    let source = serde_json::from_value::<WireCalendarEvent>(editable_event_json(
        "synthetic-version-1",
        "Planning",
        "2030-01-08T09:00:00.0000000",
        "2030-01-08T10:00:00.0000000",
    ))
    .expect("synthetic event deserializes")
    .into_new(view)
    .expect("synthetic cached event is complete");
    let updated = editable_event_json(
        "synthetic-version-2",
        "Updated planning",
        "2030-01-08T11:00:00.0000000",
        "2030-01-08T12:00:00.0000000",
    );
    let read = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/events/synthetic-event")
                .header("prefer", "IdType=\"ImmutableId\"")
                .query_param("$select", CALENDAR_EVENT_EDIT_SELECT);
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    editable_event_json(
                        "synthetic-version-1",
                        "Planning",
                        "2030-01-08T09:00:00.0000000",
                        "2030-01-08T10:00:00.0000000",
                    )
                    .to_string(),
                );
        })
        .await;
    let patch = server
        .mock_async(|when, then| {
            when.method(PATCH)
                .path("/v1.0/me/events/synthetic-event")
                .header("prefer", "IdType=\"ImmutableId\"")
                .query_param_missing("$select")
                .header("content-type", "application/json")
                .body(r#"{"subject":"Updated planning","start":{"dateTime":"2030-01-08T11:00:00","timeZone":"UTC"},"end":{"dateTime":"2030-01-08T12:00:00","timeZone":"UTC"}}"#);
            then.status(200)
                .header("content-type", "application/json")
                .body(updated.to_string());
        })
        .await;
    let (api, _transport) = synthetic_graph_api(&server);
    let editable = api
        .editable_calendar_event(&source, synthetic_access_token())
        .await
        .expect("synthetic supported event passes the edit check");
    let update = CalendarEventUpdate::new(
        editable,
        CalendarEventUpdateDraft::timed(
            "Updated planning".to_owned(),
            utc_time(8, 11),
            utc_time(8, 12),
        )
        .expect("synthetic update draft is valid"),
    )
    .expect("synthetic event values changed");

    let saved = api
        .update_calendar_event(&update, synthetic_access_token())
        .await
        .expect("Graph confirms the exact event update");

    assert_eq!(saved.subject(), Some("Updated planning"));
    assert_eq!(saved.location(), Some("Room A"));
    assert_eq!(saved.time(), update.draft().time());
    read.assert_calls_async(2).await;
    patch.assert_async().await;
}

#[tokio::test]
async fn event_update_refuses_a_changed_version_without_patching() {
    let server = MockServer::start_async().await;
    let source = editable_event("synthetic-version-1");
    let update = CalendarEventUpdate::new(
        source,
        CalendarEventUpdateDraft::timed(
            "Updated planning".to_owned(),
            utc_time(8, 11),
            utc_time(8, 12),
        )
        .expect("synthetic update draft is valid"),
    )
    .expect("synthetic event values changed");
    let read = server
        .mock_async(|when, then| {
            when.method(GET).path("/v1.0/me/events/synthetic-event");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    editable_event_json(
                        "synthetic-version-2",
                        "Concurrent change",
                        "2030-01-08T09:00:00.0000000",
                        "2030-01-08T10:00:00.0000000",
                    )
                    .to_string(),
                );
        })
        .await;
    let patch = server
        .mock_async(|when, then| {
            when.method(PATCH).path("/v1.0/me/events/synthetic-event");
            then.status(200).body("{}");
        })
        .await;
    let (api, _transport) = synthetic_graph_api(&server);

    assert_eq!(
        api.update_calendar_event(&update, synthetic_access_token())
            .await,
        Err(GraphError::CalendarEventChanged)
    );
    read.assert_calls_async(1).await;
    patch.assert_calls_async(0).await;
}

#[tokio::test]
async fn event_update_refuses_a_missing_event_without_patching() {
    let server = MockServer::start_async().await;
    let update = CalendarEventUpdate::new(
        editable_event("synthetic-version-1"),
        CalendarEventUpdateDraft::timed(
            "Updated planning".to_owned(),
            utc_time(8, 11),
            utc_time(8, 12),
        )
        .expect("synthetic update draft is valid"),
    )
    .expect("synthetic event values changed");
    let read = server
        .mock_async(|when, then| {
            when.method(GET).path("/v1.0/me/events/synthetic-event");
            then.status(404);
        })
        .await;
    let patch = server
        .mock_async(|when, then| {
            when.method(PATCH).path("/v1.0/me/events/synthetic-event");
            then.status(200).body("{}");
        })
        .await;
    let (api, _transport) = synthetic_graph_api(&server);

    assert_eq!(
        api.update_calendar_event(&update, synthetic_access_token())
            .await,
        Err(GraphError::CalendarEventChanged)
    );
    read.assert_calls_async(1).await;
    patch.assert_calls_async(0).await;
}

#[tokio::test]
async fn event_delete_rechecks_version_and_sends_the_current_etag() {
    let server = MockServer::start_async().await;
    let event = editable_event("synthetic-version-1");
    let read = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/events/synthetic-event")
                .header("prefer", "IdType=\"ImmutableId\"")
                .query_param("$select", CALENDAR_EVENT_EDIT_SELECT);
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    editable_event_json(
                        "synthetic-version-1",
                        "Planning",
                        "2030-01-08T09:00:00.0000000",
                        "2030-01-08T10:00:00.0000000",
                    )
                    .to_string(),
                );
        })
        .await;
    let delete = server
        .mock_async(|when, then| {
            when.method(DELETE)
                .path("/v1.0/me/events/synthetic-event")
                .header("prefer", "IdType=\"ImmutableId\"")
                .header("if-match", "W/\"synthetic-etag-1\"");
            then.status(204);
        })
        .await;
    let (api, _transport) = synthetic_graph_api(&server);

    api.delete_calendar_event(&event, synthetic_access_token())
        .await
        .expect("Graph confirms the conditional delete");

    read.assert_calls_async(1).await;
    delete.assert_calls_async(1).await;
}

#[tokio::test]
async fn event_delete_refuses_a_changed_version_without_deleting() {
    let server = MockServer::start_async().await;
    let event = editable_event("synthetic-version-1");
    let read = server
        .mock_async(|when, then| {
            when.method(GET).path("/v1.0/me/events/synthetic-event");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    editable_event_json(
                        "synthetic-version-2",
                        "Changed planning",
                        "2030-01-08T09:00:00.0000000",
                        "2030-01-08T10:00:00.0000000",
                    )
                    .to_string(),
                );
        })
        .await;
    let delete = server
        .mock_async(|when, then| {
            when.method(DELETE).path("/v1.0/me/events/synthetic-event");
            then.status(204);
        })
        .await;
    let (api, _transport) = synthetic_graph_api(&server);

    assert_eq!(
        api.delete_calendar_event(&event, synthetic_access_token())
            .await,
        Err(GraphError::CalendarEventChanged)
    );
    read.assert_calls_async(1).await;
    delete.assert_calls_async(0).await;
}

#[tokio::test]
async fn event_delete_requires_an_etag_before_sending() {
    let server = MockServer::start_async().await;
    let event = editable_event("synthetic-version-1");
    let mut current = editable_event_json(
        "synthetic-version-1",
        "Planning",
        "2030-01-08T09:00:00.0000000",
        "2030-01-08T10:00:00.0000000",
    );
    current
        .as_object_mut()
        .expect("synthetic event JSON is an object")
        .remove("@odata.etag");
    let read = server
        .mock_async(|when, then| {
            when.method(GET).path("/v1.0/me/events/synthetic-event");
            then.status(200)
                .header("content-type", "application/json")
                .body(current.to_string());
        })
        .await;
    let delete = server
        .mock_async(|when, then| {
            when.method(DELETE).path("/v1.0/me/events/synthetic-event");
            then.status(204);
        })
        .await;
    let (api, _transport) = synthetic_graph_api(&server);

    assert_eq!(
        api.delete_calendar_event(&event, synthetic_access_token())
            .await,
        Err(GraphError::InvalidResponse)
    );
    read.assert_calls_async(1).await;
    delete.assert_calls_async(0).await;
}

#[tokio::test]
async fn calendar_delta_uses_exact_week_bounds_and_maps_sparse_followup_changes() {
    let server = MockServer::start_async().await;
    let next_link = server.url("/v1.0/me/calendarView/delta?$skiptoken=synthetic-next");
    let final_link = server.url("/v1.0/me/calendarView/delta?$deltatoken=synthetic-final");
    let initial_page = json!({
        "value": [{
            "id": "synthetic-event",
            "subject": "Synthetic all-day appointment",
            "location": {"displayName": "Room 4"},
            "start": {
                "dateTime": "2030-01-07T00:00:00.0000000",
                "timeZone": "UTC"
            },
            "end": {
                "dateTime": "2030-01-08T00:00:00.0000000",
                "timeZone": "UTC"
            },
            "isAllDay": true,
            "originalStartTimeZone": "Pacific Standard Time",
            "originalEndTimeZone": "Pacific Standard Time"
        }],
        "@odata.nextLink": next_link.clone()
    });
    let continuation_page = json!({
        "value": [{
            "id": "synthetic-event",
            "location": null
        }],
        "@odata.deltaLink": final_link.clone()
    });
    let first_request = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/calendarView/delta")
                .query_param("startDateTime", "2030-01-07T00:00:00Z")
                .query_param("endDateTime", "2030-01-14T00:00:00Z")
                .header("content-type", "application/json")
                .header("prefer", "IdType=\"ImmutableId\"");
            then.status(200)
                .header("content-type", "application/json")
                .body(initial_page.to_string());
        })
        .await;
    let continuation_request = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/calendarView/delta")
                .query_param("$skiptoken", "synthetic-next")
                .header("content-type", "application/json")
                .header("prefer", "IdType=\"ImmutableId\"");
            then.status(200)
                .header("content-type", "application/json")
                .body(continuation_page.to_string());
        })
        .await;
    let (api, _transport) = synthetic_graph_api(&server);
    let view = synthetic_view();
    let token = synthetic_access_token();

    let initial = api
        .calendar_delta_page(&view, CalendarDeltaRequest::Initial, Arc::clone(&token))
        .await
        .expect("synthetic bounded calendar delta request succeeds");
    let CalendarDeltaResponse::Page(initial) = initial else {
        panic!("initial request returns a page");
    };
    let (initial_page, initial_response_bytes) = initial.into_parts();
    assert!(initial_response_bytes > 0);
    let (mut changes, progress) = initial_page.into_parts();
    let next = match progress {
        CalendarDeltaProgress::Next(link) => link,
        CalendarDeltaProgress::Complete(_) => panic!("initial response must have a next link"),
    };
    assert_eq!(next.as_str(), next_link);
    let CalendarDeltaChange::Upsert { id, patch } = changes.remove(0) else {
        panic!("synthetic event must be an upsert");
    };
    let mut occurrence: CalendarOccurrence = patch
        .into_new(view.clone(), id)
        .expect("new Graph occurrence includes a complete date range");
    assert_eq!(occurrence.subject(), Some("Synthetic all-day appointment"));
    assert_eq!(occurrence.location(), Some("Room 4"));
    assert_eq!(
        occurrence.original_start_time_zone(),
        Some("Pacific Standard Time")
    );
    assert!(matches!(
        occurrence.time(),
        CalendarOccurrenceTime::AllDay { start, end_exclusive }
            if *start == Date::from_calendar_date(2030, Month::January, 7).expect("valid date")
                && *end_exclusive == Date::from_calendar_date(2030, Month::January, 8).expect("valid date")
    ));

    let continuation = api
        .calendar_delta_page(&view, CalendarDeltaRequest::Continue(next), token)
        .await
        .expect("synthetic continuation request succeeds");
    let CalendarDeltaResponse::Page(continuation) = continuation else {
        panic!("continuation request returns a page");
    };
    let (continuation_page, _) = continuation.into_parts();
    let (mut changes, progress) = continuation_page.into_parts();
    assert!(
        matches!(progress, CalendarDeltaProgress::Complete(ref link) if link.as_str() == final_link)
    );
    let CalendarDeltaChange::Upsert { patch, .. } = changes.remove(0) else {
        panic!("synthetic sparse update must be an upsert");
    };
    patch
        .apply_to(&mut occurrence)
        .expect("sparse update preserves the existing occurrence range");
    assert_eq!(occurrence.subject(), Some("Synthetic all-day appointment"));
    assert_eq!(occurrence.location(), None);
    assert!(matches!(
        occurrence.time(),
        CalendarOccurrenceTime::AllDay { .. }
    ));

    first_request.assert_async().await;
    continuation_request.assert_async().await;
}

#[tokio::test]
async fn calendar_delta_preserves_recurring_occurrence_instants_across_dst() {
    let server = MockServer::start_async().await;
    let delta_link = server.url("/v1.0/me/calendarView/delta?$deltatoken=synthetic-dst");
    let page = json!({
        "value": [
            recurring_graph_event_json(
                "synthetic-before-dst",
                "2026-03-22T08:00:00.0000000",
                "2026-03-22T09:00:00.0000000"
            ),
            recurring_graph_event_json(
                "synthetic-after-dst",
                "2026-03-29T07:00:00.0000000",
                "2026-03-29T08:00:00.0000000"
            )
        ],
        "@odata.deltaLink": delta_link
    });
    let request = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/calendarView/delta")
                .query_param("startDateTime", "2026-03-15T23:00:00Z")
                .query_param("endDateTime", "2026-03-29T22:00:00Z")
                .header("content-type", "application/json")
                .header("prefer", "IdType=\"ImmutableId\"");
            then.status(200)
                .header("content-type", "application/json")
                .body(page.to_string());
        })
        .await;
    let (api, _transport) = synthetic_graph_api(&server);
    let account =
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid");
    let start_date = Date::from_calendar_date(2026, Month::March, 16)
        .expect("synthetic calendar start date is valid");
    let end_date = Date::from_calendar_date(2026, Month::March, 30)
        .expect("synthetic calendar end date is valid");
    let range = CalendarViewRange::new(
        synthetic_utc_datetime("2026-03-15T23:00:00Z"),
        synthetic_utc_datetime("2026-03-29T22:00:00Z"),
        start_date,
        end_date,
    )
    .expect("synthetic calendar range spans the DST transition");
    let view = CalendarViewReference::new(account, range);

    let response = api
        .calendar_delta_page(
            &view,
            CalendarDeltaRequest::Initial,
            synthetic_access_token(),
        )
        .await
        .expect("synthetic calendar delta request succeeds");
    let CalendarDeltaResponse::Page(page) = response else {
        panic!("synthetic initial calendar response contains a page");
    };
    let (page, response_bytes) = page.into_parts();
    assert!(response_bytes > 0, "Graph returns a non-empty delta page");
    let (changes, progress) = page.into_parts();
    assert!(
        matches!(progress, CalendarDeltaProgress::Complete(_)),
        "synthetic initial response ends with a delta link"
    );
    assert_eq!(changes.len(), 2, "Graph expands both weekly occurrences");
    let mut changes = changes.into_iter();
    let before_dst = occurrence_from_graph_change(
        changes
            .next()
            .expect("the response contains a pre-DST occurrence"),
        &view,
    );
    let after_dst = occurrence_from_graph_change(
        changes
            .next()
            .expect("the response contains a post-DST occurrence"),
        &view,
    );
    assert!(
        changes.next().is_none(),
        "the response contains two occurrences"
    );

    assert_dst_occurrence(
        &before_dst,
        "synthetic-before-dst",
        "2026-03-22T08:00:00Z",
        "2026-03-22T09:00:00Z",
    );
    assert_dst_occurrence(
        &after_dst,
        "synthetic-after-dst",
        "2026-03-29T07:00:00Z",
        "2026-03-29T08:00:00Z",
    );
    request.assert_async().await;
}

#[tokio::test]
async fn calendar_410_reset_requires_a_valid_location_on_a_continuation() {
    let server = MockServer::start_async().await;
    let reset_link = format!(
        "{}/v1.0/me/calendarView/delta?startDateTime=2030-01-07T00%3A00%3A00Z&endDateTime=2030-01-14T00%3A00%3A00Z&$deltatoken=",
        server.base_url()
    );
    let mock_reset_link = reset_link.clone();
    let expired = server
        .mock_async(move |when, then| {
            when.method(GET)
                .path("/v1.0/me/calendarView/delta")
                .query_param("$deltatoken", "expired")
                .header("content-type", "application/json");
            then.status(410).header("Location", mock_reset_link.clone());
        })
        .await;
    let (api, _transport) = synthetic_graph_api(&server);
    let response = api
        .calendar_delta_page(
            &synthetic_view(),
            CalendarDeltaRequest::Continue(CalendarDeltaLink::from_opaque(format!(
                "{}/v1.0/me/calendarView/delta?$deltatoken=expired",
                server.base_url()
            ))),
            synthetic_access_token(),
        )
        .await
        .expect("documented 410 reset URL is returned after validation");

    assert_eq!(
        response,
        CalendarDeltaResponse::Reset {
            reset_link: Some(CalendarDeltaLink::from_opaque(reset_link.clone())),
            response_bytes: reset_link.len(),
        }
    );
    expired.assert_async().await;
}

#[tokio::test]
async fn sync_state_not_found_starts_the_fixed_range_without_a_location() {
    let server = MockServer::start_async().await;
    let body = r#"{"error":{"code":"syncStateNotFound"}}"#;
    let expired = server
        .mock_async(move |when, then| {
            when.method(GET)
                .path("/v1.0/me/calendarView/delta")
                .query_param("$deltatoken", "expired")
                .header("content-type", "application/json");
            then.status(400).body(body);
        })
        .await;
    let (api, _transport) = synthetic_graph_api(&server);
    let response = api
        .calendar_delta_page(
            &synthetic_view(),
            CalendarDeltaRequest::Continue(CalendarDeltaLink::from_opaque(format!(
                "{}/v1.0/me/calendarView/delta?$deltatoken=expired",
                server.base_url()
            ))),
            synthetic_access_token(),
        )
        .await
        .expect("documented expiration code is mapped without returning the error body");

    assert_eq!(
        response,
        CalendarDeltaResponse::Reset {
            reset_link: None,
            response_bytes: body.len(),
        }
    );
    expired.assert_async().await;
}

#[test]
fn rejects_non_utc_and_explicit_offset_calendar_times() {
    for (date_time, time_zone) in [
        ("2030-01-07T09:00:00.0000000", "Pacific Standard Time"),
        ("2030-01-07T09:00:00+01:00", "UTC"),
    ] {
        let event: WireCalendarEvent = serde_json::from_value(json!({
            "id": "synthetic-event",
            "start": {"dateTime": date_time, "timeZone": time_zone}
        }))
        .expect("synthetic event wire fields are valid JSON");
        assert_eq!(event.into_change(), Err(GraphError::InvalidRecord));
    }
}

#[test]
fn created_event_response_requires_a_complete_timed_range() {
    let event: WireCalendarEvent = serde_json::from_value(json!({
        "id": "synthetic-event",
        "subject": "Synthetic planning meeting",
        "isAllDay": false
    }))
    .expect("synthetic incomplete event is valid JSON");

    assert_eq!(
        event.into_new(synthetic_view()),
        Err(GraphError::InvalidRecord)
    );
}

#[test]
fn pagination_budget_accepts_limits_and_rejects_additional_work() {
    let mut budget = PaginationBudget::default();
    for _ in 0..MAX_PAGINATION_PAGES {
        budget
            .begin_page()
            .expect("page is within the configured limit");
    }
    assert_eq!(
        budget.begin_page(),
        Err(GraphError::PaginationLimitExceeded)
    );

    let mut byte_budget = PaginationBudget::default();
    byte_budget
        .add_response_bytes(MAX_PAGINATION_RESPONSE_BYTES)
        .expect("response bytes are within the configured limit");
    assert_eq!(
        byte_budget.add_response_bytes(1),
        Err(GraphError::PaginationLimitExceeded)
    );
}

#[test]
fn missing_hidden_folder_flag_defaults_to_visible() {
    let folder: WireFolder = serde_json::from_value(json!({
        "id": "folder-1",
        "displayName": "Inbox"
    }))
    .expect("synthetic folder without isHidden is valid");

    assert!(!folder.is_hidden);
}

#[test]
fn maps_folder_payload_to_account_scoped_domain_values() {
    let account_id = AccountId::try_from("synthetic-account".to_owned())
        .expect("synthetic account ID is non-empty");
    let folder = WireFolder {
        id: "folder-1".to_owned(),
        display_name: "Inbox".to_owned(),
        is_hidden: false,
    }
    .into_domain(&account_id, None)
    .expect("synthetic folder maps to the domain")
    .expect("synthetic folder is visible");
    assert_eq!(folder.reference().account_id(), &account_id);
}

#[test]
fn rejects_empty_folder_ids() {
    let account_id = AccountId::try_from("synthetic-account".to_owned())
        .expect("synthetic account ID is non-empty");
    assert!(
        WireFolder {
            id: String::new(),
            display_name: "Inbox".to_owned(),
            is_hidden: false,
        }
        .into_domain(&account_id, None)
        .is_err()
    );
}

#[test]
fn rejects_folder_metadata_that_lists_itself_as_parent() {
    let account_id = AccountId::try_from("synthetic-account".to_owned())
        .expect("synthetic account ID is non-empty");
    let parent_id =
        FolderId::try_from("folder-1".to_owned()).expect("synthetic folder ID is non-empty");
    let folder = WireFolder {
        id: "folder-1".to_owned(),
        display_name: "Child".to_owned(),
        is_hidden: false,
    }
    .into_domain(&account_id, Some(parent_id));

    assert_eq!(folder.err(), Some(GraphError::InvalidRecord));
}

#[test]
fn excludes_hidden_folders_from_the_browsable_tree() {
    let account_id = AccountId::try_from("synthetic-account".to_owned())
        .expect("synthetic account ID is non-empty");
    let folder = WireFolder {
        id: "folder-hidden".to_owned(),
        display_name: "Hidden".to_owned(),
        is_hidden: true,
    }
    .into_domain(&account_id, None)
    .expect("hidden folder metadata is valid");

    assert!(folder.is_none());
}
