//! In-memory coroutine tests: each coroutine runs against a scripted
//! HTTP response (no network), checking the request it writes and the
//! value it returns.

mod common;

use common::{empty_response, json_response, run, text_response};
use io_http::rfc6750::bearer::HttpAuthBearer;
use io_msgraph::v1::{
    field::MsgraphField,
    query::to_query_pairs,
    rest::batch::{MsgraphBatch, MsgraphBatchRequest, MsgraphBatchResponses},
    rest::users::{
        contact_folders::{
            MsgraphContactFolder, create::MsgraphContactFolderCreate,
            delete::MsgraphContactFolderDelete, list::MsgraphContactFoldersList,
        },
        contacts::{
            MsgraphContact, create::MsgraphContactCreate, delete::MsgraphContactDelete,
            delta::MsgraphContactsDelta, list::MsgraphContactsList,
            list::MsgraphContactsListParams, update::MsgraphContactUpdate,
        },
        get::MsgraphUserGet,
        mail_folders::{
            MsgraphMailFolder, create::MsgraphMailFolderCreate, delete::MsgraphMailFolderDelete,
            list::MsgraphMailFoldersList, list::MsgraphMailFoldersListParams,
            update::MsgraphMailFolderUpdate,
        },
        messages::{
            MsgraphEmailAddress, MsgraphMessage,
            attachments::{
                delete::MsgraphAttachmentDelete, get_raw::MsgraphAttachmentGetRaw,
                list::MsgraphAttachmentsList,
            },
            create_mime::MsgraphMessageCreateMime,
            delta::{MsgraphMessagesDelta, MsgraphMessagesDeltaParams},
            get_raw::MsgraphMessageGetRaw,
            list::MsgraphMessagesList,
            list::MsgraphMessagesListParams,
            r#move::MsgraphMessageMove,
            update::MsgraphMessageUpdate,
        },
        send_mail::MsgraphMailSendMime,
    },
    send::{MsgraphSendError, parse_api_error},
};

fn auth() -> HttpAuthBearer {
    HttpAuthBearer::new("test-token")
}

#[test]
fn gets_user() {
    let body = r#"{"id":"abc","displayName":"Alice","mail":"alice@example.com","userPrincipalName":"alice@example.com"}"#;

    let mut coroutine = MsgraphUserGet::new(&auth(), "me").unwrap();
    let (result, written) = run(&mut coroutine, &json_response("HTTP/1.1 200 OK", body));
    let out = result.unwrap();

    let request = String::from_utf8_lossy(&written);
    assert!(request.starts_with("GET /v1.0/me"));
    assert!(request.contains("Authorization: Bearer test-token"));
    assert_eq!(out.response.mail.as_deref(), Some("alice@example.com"));
}

#[test]
fn mail_folders_list_parses_value() {
    let body = r#"{
        "value": [
            { "id": "AAA", "displayName": "Inbox", "totalItemCount": 71, "unreadItemCount": 70 },
            { "id": "BBB", "displayName": "Archive" }
        ]
    }"#;

    let mut coroutine = MsgraphMailFoldersList::new(&auth(), "me", &Default::default()).unwrap();
    let (result, written) = run(&mut coroutine, &json_response("HTTP/1.1 200 OK", body));
    let out = result.unwrap();

    let request = String::from_utf8_lossy(&written);
    assert!(request.starts_with("GET /v1.0/me/mailFolders"));

    assert_eq!(out.response.value.len(), 2);
    assert_eq!(out.response.value[0].id, "AAA");
    assert_eq!(out.response.value[0].display_name, "Inbox");
    assert_eq!(out.response.value[0].total_item_count, Some(71));
    assert_eq!(out.response.value[1].display_name, "Archive");
}

#[test]
fn batch_posts_requests_and_parses_responses() {
    let body = r#"{
        "responses": [
            { "id": "2", "status": 404, "body": { "error": { "code": "ErrorFolderNotFound", "message": "Not found" } } },
            { "id": "1", "status": 200, "headers": { "Content-Type": "application/json" }, "body": { "id": "AAA", "displayName": "Inbox" } }
        ]
    }"#;

    let requests = [
        MsgraphBatchRequest {
            id: "1".into(),
            method: "GET".into(),
            url: "/me/mailFolders/inbox".into(),
            ..Default::default()
        },
        MsgraphBatchRequest {
            id: "2".into(),
            method: "GET".into(),
            url: "/me/mailFolders/archive".into(),
            ..Default::default()
        },
    ];
    let mut coroutine = MsgraphBatch::new(&auth(), &requests).unwrap();
    let (result, written) = run(&mut coroutine, &json_response("HTTP/1.1 200 OK", body));
    let mut responses = result.unwrap().response.responses;

    let request = String::from_utf8_lossy(&written);
    assert!(request.starts_with("POST /v1.0/$batch"), "got: {request}");
    assert!(
        request.contains(r#"{"requests":[{"id":"1","method":"GET","url":"/me/mailFolders/inbox"}"#),
        "got: {request}"
    );

    let folder: MsgraphMailFolder = responses.pop().unwrap().parse().unwrap();
    assert_eq!(folder.id, "AAA");

    let err = responses.pop().unwrap().parse::<MsgraphMailFolder>();
    assert!(
        matches!(err, Err(MsgraphSendError::Api { status: 404, ref code, .. }) if code == "ErrorFolderNotFound")
    );
}

#[test]
fn batch_response_decodes_base64_bodies_and_retry_after() {
    let body = r#"{
        "responses": [
            { "id": "1", "status": 200, "headers": { "Content-Type": "message/rfc822" }, "body": "U3ViamVjdDogSGkNCg0KSGVsbG8=" },
            { "id": "2", "status": 204, "body": null },
            { "id": "3", "status": 200, "body": "not base64!" },
            { "id": "4", "status": 200, "body": { "id": "AAA" } },
            { "id": "5", "status": 429, "headers": { "Retry-After": "7" }, "body": { "error": { "code": "TooManyRequests", "message": "Throttled" } } },
            { "id": "6", "status": 503, "headers": { "retry-after": "Wed, 21 Oct 2026 07:28:00 GMT" } },
            { "id": "7", "status": 200, "body": "U3ViamVjdDogSGkNCg0K\r\nSGVsbG8" },
            { "id": "8", "status": 200, "body": "U3ViamVjdDogSGkNCg0K*SGVsbG8=" }
        ]
    }"#;

    let responses: MsgraphBatchResponses = serde_json::from_str(body).unwrap();
    let mut responses = responses.responses.into_iter();
    let mut next = || responses.next().unwrap();

    assert_eq!(next().bytes().unwrap(), b"Subject: Hi\r\n\r\nHello");
    assert!(next().bytes().unwrap().is_empty());
    assert!(matches!(
        next().bytes(),
        Err(MsgraphSendError::ParseResponse(_))
    ));
    assert!(matches!(
        next().bytes(),
        Err(MsgraphSendError::ParseResponse(_))
    ));

    let throttled = next();
    assert_eq!(throttled.retry_after(), Some(7));
    assert!(matches!(
        throttled.bytes(),
        Err(MsgraphSendError::Api { status: 429, .. })
    ));

    assert_eq!(next().retry_after(), None);

    // NOTE: line breaks and missing padding decode, a stray byte fails
    assert_eq!(next().bytes().unwrap(), b"Subject: Hi\r\n\r\nHello");
    assert!(matches!(
        next().bytes(),
        Err(MsgraphSendError::ParseResponse(_))
    ));
}

#[test]
fn batch_rejects_empty_oversized_and_duplicate_ids() {
    let request = |id: &str| MsgraphBatchRequest {
        id: id.into(),
        method: "GET".into(),
        url: "/me".into(),
        ..Default::default()
    };

    let empty = MsgraphBatch::new(&auth(), &[]);
    assert!(matches!(empty, Err(MsgraphSendError::InvalidRequest(_))));

    let oversized: Vec<_> = (0..21).map(|i| request(&i.to_string())).collect();
    let oversized = MsgraphBatch::new(&auth(), &oversized);
    assert!(matches!(
        oversized,
        Err(MsgraphSendError::InvalidRequest(_))
    ));

    let duplicate = MsgraphBatch::new(&auth(), &[request("1"), request("1")]);
    assert!(matches!(
        duplicate,
        Err(MsgraphSendError::InvalidRequest(_))
    ));
}

#[test]
fn creates_folder_posts_display_name() {
    let body = r#"{ "id": "AAA", "displayName": "todo" }"#;

    let folder = MsgraphMailFolder {
        display_name: "todo".into(),
        ..Default::default()
    };
    let mut coroutine = MsgraphMailFolderCreate::new(&auth(), "me", &folder).unwrap();
    let (result, written) = run(&mut coroutine, &json_response("HTTP/1.1 201 Created", body));

    assert_eq!(result.unwrap().response.id, "AAA");

    let request = String::from_utf8_lossy(&written);
    assert!(request.starts_with("POST /v1.0/me/mailFolders"));
    assert!(
        request.contains("\"displayName\":\"todo\""),
        "got: {request}"
    );
}

#[test]
fn rejects_empty_folder_name() {
    let folder = MsgraphMailFolder {
        display_name: "  ".into(),
        ..Default::default()
    };
    let result = MsgraphMailFolderCreate::new(&auth(), "me", &folder);
    assert!(matches!(result, Err(MsgraphSendError::InvalidRequest(_))));
}

#[test]
fn deletes_folder_issues_delete() {
    let mut coroutine = MsgraphMailFolderDelete::new(&auth(), "me", "AAA").unwrap();
    let (result, written) = run(&mut coroutine, &empty_response("HTTP/1.1 204 No Content"));

    result.unwrap();

    let request = String::from_utf8_lossy(&written);
    assert!(
        request.starts_with("DELETE /v1.0/me/mailFolders/AAA"),
        "got: {request}"
    );
}

#[test]
fn messages_list_builds_odata_query() {
    let params = MsgraphMessagesListParams {
        top: Some(2),
        select: Some("id,subject,isRead"),
        orderby: Some("receivedDateTime desc"),
        ..Default::default()
    };

    let mut coroutine = MsgraphMessagesList::new(&auth(), "me", Some("inbox"), &params).unwrap();
    let (result, written) = run(
        &mut coroutine,
        &json_response("HTTP/1.1 200 OK", r#"{ "value": [] }"#),
    );
    assert!(result.is_ok());

    let request = String::from_utf8_lossy(&written);
    assert!(request.contains("/v1.0/me/mailFolders/inbox/messages"));
    assert!(request.contains("%24top=2"));
    assert!(request.contains("%24select=id%2Csubject%2CisRead"));
    assert!(request.contains("%24orderby=receivedDateTime+desc"));
    // NOTE: unset params do not appear
    assert!(!request.contains("%24skip"));
    assert!(!request.contains("%24filter"));
    assert!(!request.contains("%24expand"));
}

#[test]
fn messages_list_expands_an_extended_property() {
    let body = r#"{
        "value": [
            {
                "id": "MSG1",
                "singleValueExtendedProperties": [
                    { "id": "Integer 0xe08", "value": "48213" }
                ]
            },
            { "id": "MSG2" }
        ]
    }"#;
    let params = MsgraphMessagesListParams {
        expand: Some("singleValueExtendedProperties($filter=id eq 'Integer 0x0E08')"),
        ..Default::default()
    };

    let mut coroutine = MsgraphMessagesList::new(&auth(), "me", Some("inbox"), &params).unwrap();
    let (result, written) = run(&mut coroutine, &json_response("HTTP/1.1 200 OK", body));
    let value = result.unwrap().response.value;

    let request = String::from_utf8_lossy(&written);
    assert!(
        request.contains(
            "%24expand=singleValueExtendedProperties%28%24filter%3Did+eq+%27Integer+0x0E08%27%29"
        ),
        "got: {request}"
    );
    assert_eq!(value[0].single_value_extended_properties[0].value, "48213");
    assert!(value[1].single_value_extended_properties.is_empty());
}

#[test]
fn messages_delta_parses_changes_and_removals() {
    let body = r#"{
        "value": [
            {
                "id": "MSG1",
                "subject": "hello",
                "isRead": false,
                "receivedDateTime": "2026-08-06T10:00:00Z",
                "parentFolderId": "INBOX1",
                "from": { "emailAddress": { "name": "Alice", "address": "alice@example.com" } }
            },
            { "id": "MSG2", "@removed": { "reason": "deleted" } }
        ],
        "@odata.nextLink": "https://graph.microsoft.com/v1.0/me/messages/delta?$skiptoken=abc"
    }"#;

    let mut coroutine = MsgraphMessagesDelta::new(&auth(), "me", None, None).unwrap();
    let (result, written) = run(&mut coroutine, &json_response("HTTP/1.1 200 OK", body));
    let out = result.unwrap();

    let request = String::from_utf8_lossy(&written);
    assert!(
        request.starts_with("GET /v1.0/me/messages/delta"),
        "got: {request}"
    );

    assert_eq!(out.response.value.len(), 2);
    let changed = &out.response.value[0];
    assert_eq!(changed.message.id, "MSG1");
    assert_eq!(changed.message.subject.as_deref(), Some("hello"));
    assert_eq!(changed.message.is_read, Some(false));
    assert_eq!(
        changed.message.received_date_time.as_deref(),
        Some("2026-08-06T10:00:00Z")
    );
    assert_eq!(changed.message.parent_folder_id.as_deref(), Some("INBOX1"));
    assert_eq!(
        changed
            .message
            .from
            .as_ref()
            .and_then(|from| from.email_address.address.as_deref()),
        Some("alice@example.com")
    );
    assert!(changed.removed.is_none());

    let removed = &out.response.value[1];
    assert_eq!(removed.message.id, "MSG2");
    assert_eq!(removed.removed.as_ref().unwrap().reason, "deleted");

    assert_eq!(
        out.response.next_link.as_deref(),
        Some("https://graph.microsoft.com/v1.0/me/messages/delta?$skiptoken=abc")
    );
    assert!(out.response.delta_link.is_none());
}

#[test]
fn messages_delta_from_link_closes_round_with_delta_link() {
    let body = r#"{
        "value": [],
        "@odata.deltaLink": "https://graph.microsoft.com/v1.0/me/messages/delta?$deltatoken=xyz"
    }"#;

    let link = "https://graph.microsoft.com/v1.0/me/messages/delta?$skiptoken=abc";
    let mut coroutine = MsgraphMessagesDelta::from_link(&auth(), link).unwrap();
    let (result, written) = run(&mut coroutine, &json_response("HTTP/1.1 200 OK", body));
    let out = result.unwrap();

    let request = String::from_utf8_lossy(&written);
    assert!(
        request.contains("/v1.0/me/messages/delta"),
        "got: {request}"
    );
    assert!(request.contains("skiptoken=abc"), "got: {request}");

    assert!(out.response.value.is_empty());
    assert!(out.response.next_link.is_none());
    assert_eq!(
        out.response.delta_link.as_deref(),
        Some("https://graph.microsoft.com/v1.0/me/messages/delta?$deltatoken=xyz")
    );
}

#[test]
fn messages_delta_builds_folder_scoped_url_with_select() {
    let mut coroutine =
        MsgraphMessagesDelta::new(&auth(), "me", Some("inbox"), Some("id,subject,isRead")).unwrap();
    let (result, written) = run(
        &mut coroutine,
        &json_response("HTTP/1.1 200 OK", r#"{ "value": [] }"#),
    );
    assert!(result.is_ok());

    let request = String::from_utf8_lossy(&written);
    assert!(
        request.starts_with("GET /v1.0/me/mailFolders/inbox/messages/delta"),
        "got: {request}"
    );
    assert!(
        request.contains("%24select=id%2Csubject%2CisRead"),
        "got: {request}"
    );
}

#[test]
fn messages_delta_addresses_explicit_user() {
    let mut coroutine = MsgraphMessagesDelta::new(&auth(), "USER1", None, None).unwrap();
    let (result, written) = run(
        &mut coroutine,
        &json_response("HTTP/1.1 200 OK", r#"{ "value": [] }"#),
    );
    assert!(result.is_ok());

    let request = String::from_utf8_lossy(&written);
    assert!(
        request.starts_with("GET /v1.0/users/USER1/messages/delta"),
        "got: {request}"
    );
}

#[test]
fn messages_delta_prefers_page_size_on_first_and_next_requests() {
    let params = MsgraphMessagesDeltaParams {
        select: Some("id,subject,receivedDateTime"),
        filter: Some("receivedDateTime ge 2026-01-01T00:00:00Z"),
        expand: Some("singleValueExtendedProperties($filter=id eq 'Integer 0x0E08')"),
        max_page_size: Some(1000),
    };
    let mut coroutine =
        MsgraphMessagesDelta::with_params(&auth(), "me", Some("inbox"), &params).unwrap();
    let (result, written) = run(
        &mut coroutine,
        &json_response("HTTP/1.1 200 OK", r#"{ "value": [] }"#),
    );
    assert!(result.is_ok());

    let request = String::from_utf8_lossy(&written);
    assert!(
        request.starts_with("GET /v1.0/me/mailFolders/inbox/messages/delta?"),
        "got: {request}"
    );
    assert!(
        request.contains("%24select=id%2Csubject%2CreceivedDateTime"),
        "got: {request}"
    );
    assert!(
        request.contains("%24filter=receivedDateTime+ge+2026-01-01T00%3A00%3A00Z"),
        "got: {request}"
    );
    assert!(
        request.contains("%24expand=singleValueExtendedProperties"),
        "got: {request}"
    );
    // NOTE: the page size is a header, never a query option
    assert!(!request.contains("max_page_size"), "got: {request}");
    assert!(
        request.contains("\r\nPrefer: odata.maxpagesize=1000\r\n"),
        "got: {request}"
    );

    let link =
        "https://graph.microsoft.com/v1.0/me/mailFolders/inbox/messages/delta?$skiptoken=abc";
    let mut coroutine = MsgraphMessagesDelta::from_link(&auth(), link)
        .unwrap()
        .max_page_size(1000);
    let (result, written) = run(
        &mut coroutine,
        &json_response("HTTP/1.1 200 OK", r#"{ "value": [] }"#),
    );
    assert!(result.is_ok());

    let request = String::from_utf8_lossy(&written);
    assert!(request.contains("skiptoken=abc"), "got: {request}");
    assert!(
        request.contains("\r\nPrefer: odata.maxpagesize=1000\r\n"),
        "got: {request}"
    );
}

#[test]
fn messages_delta_prefers_nothing_unless_asked() {
    let mut coroutine =
        MsgraphMessagesDelta::new(&auth(), "me", Some("inbox"), Some("id")).unwrap();
    let (result, written) = run(
        &mut coroutine,
        &json_response("HTTP/1.1 200 OK", r#"{ "value": [] }"#),
    );
    assert!(result.is_ok());
    let request = String::from_utf8_lossy(&written);
    assert!(!request.contains("Prefer:"), "got: {request}");
    assert!(!request.contains("%24filter"), "got: {request}");

    let link = "https://graph.microsoft.com/v1.0/me/messages/delta?$skiptoken=abc";
    let mut coroutine = MsgraphMessagesDelta::from_link(&auth(), link).unwrap();
    let (result, written) = run(
        &mut coroutine,
        &json_response("HTTP/1.1 200 OK", r#"{ "value": [] }"#),
    );
    assert!(result.is_ok());
    let request = String::from_utf8_lossy(&written);
    assert!(!request.contains("Prefer:"), "got: {request}");
}

#[test]
fn contacts_and_events_delta_prefer_page_size_when_asked() {
    use io_msgraph::v1::rest::users::events::delta::MsgraphEventsDelta;

    let mut contacts = MsgraphContactsDelta::new(&auth(), "me", None, Some("id"))
        .unwrap()
        .max_page_size(500);
    let (result, written) = run(
        &mut contacts,
        &json_response("HTTP/1.1 200 OK", r#"{ "value": [] }"#),
    );
    assert!(result.is_ok());
    let request = String::from_utf8_lossy(&written);
    assert!(
        request.contains("\r\nPrefer: odata.maxpagesize=500\r\n"),
        "got: {request}"
    );

    let link = "https://graph.microsoft.com/v1.0/me/calendarView/delta?$skiptoken=abc";
    let mut events = MsgraphEventsDelta::from_link(&auth(), link)
        .unwrap()
        .max_page_size(200);
    let (result, written) = run(
        &mut events,
        &json_response("HTTP/1.1 200 OK", r#"{ "value": [] }"#),
    );
    assert!(result.is_ok());
    let request = String::from_utf8_lossy(&written);
    assert!(
        request.contains("\r\nPrefer: odata.maxpagesize=200\r\n"),
        "got: {request}"
    );
}

#[test]
fn updates_message_patches_is_read() {
    let body = r#"{ "id": "ID1", "isRead": true }"#;

    let patch = MsgraphMessage {
        is_read: Some(true),
        ..Default::default()
    };
    let mut coroutine = MsgraphMessageUpdate::new(&auth(), "me", "ID1", &patch).unwrap();
    let (result, written) = run(&mut coroutine, &json_response("HTTP/1.1 200 OK", body));

    assert_eq!(result.unwrap().response.is_read, Some(true));

    let request = String::from_utf8_lossy(&written);
    assert!(
        request.starts_with("PATCH /v1.0/me/messages/ID1"),
        "got: {request}"
    );
    assert!(request.contains("\"isRead\":true"), "got: {request}");
}

#[test]
fn move_posts_destination_id() {
    let body = r#"{ "id": "NEW" }"#;

    let mut coroutine = MsgraphMessageMove::new(&auth(), "me", "ID1", "archive").unwrap();
    let (result, written) = run(&mut coroutine, &json_response("HTTP/1.1 201 Created", body));

    assert_eq!(result.unwrap().response.id, "NEW");

    let request = String::from_utf8_lossy(&written);
    assert!(request.contains("/v1.0/me/messages/ID1/move"));
    assert!(
        request.contains("\"destinationId\":\"archive\""),
        "got: {request}"
    );
}

#[test]
fn message_get_raw_returns_mime_bytes() {
    let mime = b"From: a@b.c\r\nSubject: hi\r\n\r\nbody\r\n";

    let mut coroutine = MsgraphMessageGetRaw::new(&auth(), "me", "ID123").unwrap();
    let (result, written) = run(&mut coroutine, &text_response("HTTP/1.1 200 OK", mime));
    let out = result.unwrap();

    let request = String::from_utf8_lossy(&written);
    assert!(request.contains("/v1.0/me/messages/ID123/$value"));
    assert_eq!(out.response, mime);
}

#[test]
fn send_mail_mime_base64_encodes_body() {
    let mime = b"From: a@b.c\r\nSubject: hi\r\n\r\nbody";

    let mut coroutine = MsgraphMailSendMime::new(&auth(), "me", mime).unwrap();
    let (result, written) = run(&mut coroutine, &empty_response("HTTP/1.1 202 Accepted"));
    assert!(result.is_ok());

    let request = String::from_utf8_lossy(&written);
    assert!(request.contains("POST /v1.0/me/sendMail"));
    assert!(request.contains("Content-Type: text/plain"));
    // NOTE: base64 (standard) of the MIME, as Graph requires
    let expected = "RnJvbTogYUBiLmMNClN1YmplY3Q6IGhpDQoNCmJvZHk=";
    assert!(request.contains(expected));
}

#[test]
fn api_error_envelope_is_surfaced() {
    let body =
        r#"{ "error": { "code": "ErrorItemNotFound", "message": "The folder does not exist." } }"#;

    let mut coroutine = MsgraphMailFoldersList::new(&auth(), "me", &Default::default()).unwrap();
    let (result, _) = run(
        &mut coroutine,
        &json_response("HTTP/1.1 404 Not Found", body),
    );

    match result.unwrap_err() {
        MsgraphSendError::Api {
            status,
            code,
            message,
        } => {
            assert_eq!(status, 404);
            assert_eq!(code, "ErrorItemNotFound");
            assert_eq!(message, "The folder does not exist.");
        }
        err => panic!("unexpected error: {err:?}"),
    }
}

#[test]
fn parse_api_error_falls_back_on_missing_message_and_non_json() {
    let (status, code, message) =
        parse_api_error(404, br#"{ "error": { "code": "ErrorItemNotFound" } }"#);
    assert_eq!(status, 404);
    assert_eq!(code, "ErrorItemNotFound");
    assert_eq!(message, "unknown Microsoft Graph API error");

    let (status, code, message) = parse_api_error(502, b"upstream failure");
    assert_eq!(status, 502);
    assert_eq!(code, "unknown");
    assert_eq!(message, "upstream failure");
}

#[test]
fn updates_folder_patches_display_name() {
    let body = r#"{ "id": "AAA", "displayName": "renamed" }"#;

    let folder = MsgraphMailFolder {
        display_name: "renamed".into(),
        ..Default::default()
    };
    let mut coroutine = MsgraphMailFolderUpdate::new(&auth(), "me", "AAA", &folder).unwrap();
    let (result, written) = run(&mut coroutine, &json_response("HTTP/1.1 200 OK", body));

    assert_eq!(result.unwrap().response.display_name, "renamed");

    let request = String::from_utf8_lossy(&written);
    assert!(
        request.starts_with("PATCH /v1.0/me/mailFolders/AAA"),
        "got: {request}"
    );
    assert!(
        request.contains("\"displayName\":\"renamed\""),
        "got: {request}"
    );
}

#[test]
fn create_mime_base64_encodes_to_messages() {
    let mime = b"From: a@b.c\r\nSubject: hi\r\n\r\nbody";
    let body = r#"{ "id": "DRAFT1", "isDraft": true }"#;

    let mut coroutine = MsgraphMessageCreateMime::new(&auth(), "me", None, mime).unwrap();
    let (result, written) = run(&mut coroutine, &json_response("HTTP/1.1 201 Created", body));

    assert_eq!(result.unwrap().response.id, "DRAFT1");

    let request = String::from_utf8_lossy(&written);
    assert!(
        request.starts_with("POST /v1.0/me/messages"),
        "got: {request}"
    );
    assert!(request.contains("Content-Type: text/plain"));
    let expected = "RnJvbTogYUBiLmMNClN1YmplY3Q6IGhpDQoNCmJvZHk=";
    assert!(request.contains(expected), "got: {request}");
}

#[test]
fn lists_message_attachments() {
    let body = r#"{
        "value": [
            { "id": "ATT1", "name": "report.pdf", "contentType": "application/pdf", "size": 1024 }
        ]
    }"#;

    let mut coroutine = MsgraphAttachmentsList::new(&auth(), "me", "ID1").unwrap();
    let (result, written) = run(&mut coroutine, &json_response("HTTP/1.1 200 OK", body));
    let out = result.unwrap();

    let request = String::from_utf8_lossy(&written);
    assert!(
        request.starts_with("GET /v1.0/me/messages/ID1/attachments"),
        "got: {request}"
    );
    assert_eq!(out.response.value.len(), 1);
    assert_eq!(out.response.value[0].name.as_deref(), Some("report.pdf"));
    assert_eq!(out.response.value[0].size, Some(1024));
}

#[test]
fn attachment_get_raw_returns_bytes() {
    let content = b"%PDF-1.7 ...";

    let mut coroutine = MsgraphAttachmentGetRaw::new(&auth(), "me", "ID1", "ATT1").unwrap();
    let (result, written) = run(&mut coroutine, &text_response("HTTP/1.1 200 OK", content));
    let out = result.unwrap();

    let request = String::from_utf8_lossy(&written);
    assert!(
        request.contains("/v1.0/me/messages/ID1/attachments/ATT1/$value"),
        "got: {request}"
    );
    assert_eq!(out.response, content);
}

#[test]
fn deletes_attachment_issues_delete() {
    let mut coroutine = MsgraphAttachmentDelete::new(&auth(), "me", "ID1", "ATT1").unwrap();
    let (result, written) = run(&mut coroutine, &empty_response("HTTP/1.1 204 No Content"));

    result.unwrap();

    let request = String::from_utf8_lossy(&written);
    assert!(
        request.starts_with("DELETE /v1.0/me/messages/ID1/attachments/ATT1"),
        "got: {request}"
    );
}

#[test]
fn contact_folders_list_parses_value() {
    let body = r#"{
        "value": [
            { "id": "AAA", "displayName": "Contacts" },
            { "id": "BBB", "displayName": "Work", "parentFolderId": "AAA" }
        ]
    }"#;

    let mut coroutine = MsgraphContactFoldersList::new(&auth(), "me", &Default::default()).unwrap();
    let (result, written) = run(&mut coroutine, &json_response("HTTP/1.1 200 OK", body));
    let out = result.unwrap();

    let request = String::from_utf8_lossy(&written);
    assert!(request.starts_with("GET /v1.0/me/contactFolders"));

    assert_eq!(out.response.value.len(), 2);
    assert_eq!(out.response.value[0].id, "AAA");
    assert_eq!(out.response.value[0].display_name, "Contacts");
    assert_eq!(
        out.response.value[1].parent_folder_id.as_deref(),
        Some("AAA")
    );
}

#[test]
fn creates_contact_folder_posts_display_name() {
    let body = r#"{ "id": "AAA", "displayName": "friends" }"#;

    let folder = MsgraphContactFolder {
        display_name: "friends".into(),
        ..Default::default()
    };
    let mut coroutine = MsgraphContactFolderCreate::new(&auth(), "me", &folder).unwrap();
    let (result, written) = run(&mut coroutine, &json_response("HTTP/1.1 201 Created", body));

    assert_eq!(result.unwrap().response.id, "AAA");

    let request = String::from_utf8_lossy(&written);
    assert!(request.starts_with("POST /v1.0/me/contactFolders"));
    assert!(
        request.contains("\"displayName\":\"friends\""),
        "got: {request}"
    );
}

#[test]
fn rejects_empty_contact_folder_name() {
    let folder = MsgraphContactFolder {
        display_name: "  ".into(),
        ..Default::default()
    };
    let result = MsgraphContactFolderCreate::new(&auth(), "me", &folder);
    assert!(matches!(result, Err(MsgraphSendError::InvalidRequest(_))));
}

#[test]
fn deletes_contact_folder_issues_delete() {
    let mut coroutine = MsgraphContactFolderDelete::new(&auth(), "me", "AAA").unwrap();
    let (result, written) = run(&mut coroutine, &empty_response("HTTP/1.1 204 No Content"));

    result.unwrap();

    let request = String::from_utf8_lossy(&written);
    assert!(
        request.starts_with("DELETE /v1.0/me/contactFolders/AAA"),
        "got: {request}"
    );
}

#[test]
fn contacts_list_builds_odata_query() {
    let params = MsgraphContactsListParams {
        top: Some(5),
        select: Some("id,displayName,emailAddresses"),
        orderby: Some("displayName"),
        ..Default::default()
    };

    let mut coroutine = MsgraphContactsList::new(&auth(), "me", Some("AAA"), &params).unwrap();
    let (result, written) = run(
        &mut coroutine,
        &json_response("HTTP/1.1 200 OK", r#"{ "value": [] }"#),
    );
    assert!(result.is_ok());

    let request = String::from_utf8_lossy(&written);
    assert!(request.contains("/v1.0/me/contactFolders/AAA/contacts"));
    assert!(request.contains("%24top=5"));
    assert!(request.contains("%24select=id%2CdisplayName%2CemailAddresses"));
    assert!(request.contains("%24orderby=displayName"));
    // NOTE: unset params do not appear
    assert!(!request.contains("%24skip"));
    assert!(!request.contains("%24filter"));
}

#[test]
fn contacts_delta_parses_changes_and_removals() {
    let body = r#"{
        "value": [
            { "id": "CT1", "givenName": "Alice" },
            { "id": "CT2", "@removed": { "reason": "deleted" } }
        ],
        "@odata.deltaLink": "https://graph.microsoft.com/v1.0/me/contacts/delta?$deltatoken=xyz"
    }"#;

    let mut coroutine = MsgraphContactsDelta::new(&auth(), "me", Some("AAA"), Some("id")).unwrap();
    let (result, written) = run(&mut coroutine, &json_response("HTTP/1.1 200 OK", body));
    let out = result.unwrap();

    let request = String::from_utf8_lossy(&written);
    assert!(
        request.contains("/v1.0/me/contactFolders/AAA/contacts/delta"),
        "got: {request}"
    );
    assert!(request.contains("%24select=id"), "got: {request}");

    assert_eq!(out.response.value.len(), 2);
    assert_eq!(out.response.value[0].contact.id, "CT1");
    assert!(out.response.value[0].removed.is_none());
    assert_eq!(out.response.value[1].contact.id, "CT2");
    assert_eq!(
        out.response.value[1].removed.as_ref().unwrap().reason,
        "deleted"
    );
    assert_eq!(
        out.response.delta_link.as_deref(),
        Some("https://graph.microsoft.com/v1.0/me/contacts/delta?$deltatoken=xyz")
    );
}

#[test]
fn contacts_delta_from_link_requests_it_verbatim() {
    let link =
        "https://graph.microsoft.com/v1.0/me/contactFolders('AAA')/contacts/delta?$deltatoken=xyz";
    let mut coroutine = MsgraphContactsDelta::from_link(&auth(), link).unwrap();
    let (result, written) = run(
        &mut coroutine,
        &json_response("HTTP/1.1 200 OK", r#"{ "value": [] }"#),
    );
    assert!(result.is_ok());

    // NOTE: the link carries the folder, the select and the token, so
    // none of them are rebuilt.
    let request = String::from_utf8_lossy(&written);
    assert!(request.contains("/contacts/delta"), "got: {request}");
    assert!(request.contains("deltatoken=xyz"), "got: {request}");
}

#[test]
fn creates_contact_posts_email_addresses() {
    let body = r#"{ "id": "CT1", "givenName": "Alice" }"#;

    let contact = MsgraphContact {
        given_name: MsgraphField::Set("Alice".into()),
        email_addresses: MsgraphField::Set(vec![MsgraphEmailAddress {
            name: Some("Alice".into()),
            address: Some("alice@example.com".into()),
        }]),
        ..Default::default()
    };
    let mut coroutine = MsgraphContactCreate::new(&auth(), "me", None, &contact).unwrap();
    let (result, written) = run(&mut coroutine, &json_response("HTTP/1.1 201 Created", body));

    assert_eq!(result.unwrap().response.id, "CT1");

    let request = String::from_utf8_lossy(&written);
    assert!(
        request.starts_with("POST /v1.0/me/contacts"),
        "got: {request}"
    );
    assert!(
        request.contains("\"givenName\":\"Alice\""),
        "got: {request}"
    );
    assert!(
        request.contains(
            "\"emailAddresses\":[{\"name\":\"Alice\",\"address\":\"alice@example.com\"}]"
        ),
        "got: {request}"
    );
    // NOTE: unset fields do not appear in the JSON body
    assert!(!request.contains("\"surname\""), "got: {request}");
}

#[test]
fn updates_contact_patches_given_name() {
    let body = r#"{ "id": "CT1", "givenName": "Bob" }"#;

    let patch = MsgraphContact {
        given_name: MsgraphField::Set("Bob".into()),
        ..Default::default()
    };
    let mut coroutine = MsgraphContactUpdate::new(&auth(), "me", "CT1", &patch).unwrap();
    let (result, written) = run(&mut coroutine, &json_response("HTTP/1.1 200 OK", body));

    assert_eq!(result.unwrap().response.given_name.as_deref(), Some("Bob"));

    let request = String::from_utf8_lossy(&written);
    assert!(
        request.starts_with("PATCH /v1.0/me/contacts/CT1"),
        "got: {request}"
    );
    assert!(request.contains("\"givenName\":\"Bob\""), "got: {request}");
}

#[test]
fn deletes_contact_issues_delete() {
    let mut coroutine = MsgraphContactDelete::new(&auth(), "me", "CT1").unwrap();
    let (result, written) = run(&mut coroutine, &empty_response("HTTP/1.1 204 No Content"));

    result.unwrap();

    let request = String::from_utf8_lossy(&written);
    assert!(
        request.starts_with("DELETE /v1.0/me/contacts/CT1"),
        "got: {request}"
    );
}

#[test]
fn query_pairs_rename_to_odata_options() {
    let params = MsgraphMailFoldersListParams {
        top: Some(10),
        include_hidden_folders: Some(true),
        ..Default::default()
    };

    let pairs = to_query_pairs(&params);
    assert!(pairs.contains(&(String::from("$top"), String::from("10"))));
    assert!(pairs.contains(&(String::from("includeHiddenFolders"), String::from("true"))));
    assert_eq!(pairs.len(), 2);
}

#[test]
fn calendars_list_parses_value() {
    use io_msgraph::v1::rest::users::calendars::list::{
        MsgraphCalendarsList, MsgraphCalendarsListParams,
    };

    let body = r##"{ "value": [
        { "id": "CAL1", "name": "Calendar", "isDefaultCalendar": true, "hexColor": "#ff0000" }
    ] }"##;
    let params = MsgraphCalendarsListParams::default();
    let mut coroutine = MsgraphCalendarsList::new(&auth(), "me", &params).unwrap();
    let (result, written) = run(&mut coroutine, &json_response("HTTP/1.1 200 OK", body));
    let out = result.unwrap().response;

    let request = String::from_utf8_lossy(&written);
    assert!(
        request.starts_with("GET /v1.0/me/calendars"),
        "got: {request}"
    );
    assert_eq!(out.value[0].id, "CAL1");
    assert_eq!(out.value[0].name.as_deref(), Some("Calendar"));
    assert_eq!(out.value[0].is_default_calendar, Some(true));
}

#[test]
fn event_get_parses_a_series_master() {
    use io_msgraph::v1::rest::users::events::{
        MsgraphDayOfWeek, MsgraphEventType, MsgraphRecurrencePatternType,
        MsgraphRecurrenceRangeType, get::MsgraphEventGet,
    };

    let body = r#"{
        "id": "EV1",
        "iCalUId": "040000008200E00074C5B7101A82E008",
        "changeKey": "ck1",
        "type": "seriesMaster",
        "subject": "Stand-up",
        "isAllDay": false,
        "start": { "dateTime": "2026-08-14T09:00:00.0000000", "timeZone": "Romance Standard Time" },
        "end": { "dateTime": "2026-08-14T09:15:00.0000000", "timeZone": "Romance Standard Time" },
        "recurrence": {
            "pattern": { "type": "weekly", "interval": 1, "daysOfWeek": ["monday", "friday"], "firstDayOfWeek": "monday" },
            "range": { "type": "numbered", "startDate": "2026-08-14", "numberOfOccurrences": 10, "recurrenceTimeZone": "Romance Standard Time" }
        },
        "attendees": [
            { "type": "required", "status": { "response": "accepted" }, "emailAddress": { "name": "Bob", "address": "bob@x.org" } }
        ]
    }"#;
    let mut coroutine = MsgraphEventGet::new(&auth(), "me", "EV1", None, Some("x")).unwrap();
    let (result, written) = run(&mut coroutine, &json_response("HTTP/1.1 200 OK", body));
    let event = result.unwrap().response;

    let request = String::from_utf8_lossy(&written);
    assert!(
        request.starts_with("GET /v1.0/me/events/EV1?%24expand=x"),
        "got: {request}"
    );
    assert_eq!(event.event_type, Some(MsgraphEventType::SeriesMaster));
    assert_eq!(
        event.ical_uid.as_deref(),
        Some("040000008200E00074C5B7101A82E008")
    );
    let start = event.start.as_option().unwrap();
    assert_eq!(start.time_zone.as_deref(), Some("Romance Standard Time"));
    let recurrence = event.recurrence.as_option().unwrap();
    assert_eq!(
        recurrence.pattern.pattern_type,
        Some(MsgraphRecurrencePatternType::Weekly)
    );
    assert_eq!(
        recurrence.pattern.days_of_week,
        [MsgraphDayOfWeek::Monday, MsgraphDayOfWeek::Friday]
    );
    assert_eq!(
        recurrence.range.range_type,
        Some(MsgraphRecurrenceRangeType::Numbered)
    );
    assert_eq!(recurrence.range.number_of_occurrences, Some(10));
    assert_eq!(event.attendees.as_deref().unwrap().len(), 1);
}

#[test]
fn event_create_posts_into_the_named_calendar() {
    use io_msgraph::v1::rest::users::events::{MsgraphEvent, create::MsgraphEventCreate};

    let event = MsgraphEvent {
        subject: MsgraphField::Set("Lunch".into()),
        ..Default::default()
    };
    let mut coroutine = MsgraphEventCreate::new(&auth(), "me", Some("CAL1"), &event).unwrap();
    let (result, written) = run(
        &mut coroutine,
        &json_response("HTTP/1.1 201 Created", r#"{ "id": "EV2" }"#),
    );
    assert_eq!(result.unwrap().response.id, "EV2");

    let request = String::from_utf8_lossy(&written);
    assert!(
        request.starts_with("POST /v1.0/me/calendars/CAL1/events"),
        "got: {request}"
    );
    assert!(
        request.ends_with(r#"{"subject":"Lunch"}"#),
        "got: {request}"
    );
}

#[test]
fn event_update_patches_only_what_is_set_or_cleared() {
    use io_msgraph::v1::rest::users::events::{MsgraphEvent, update::MsgraphEventUpdate};

    let event = MsgraphEvent {
        subject: MsgraphField::Set("Lunch".into()),
        location: MsgraphField::Null,
        ..Default::default()
    };
    let mut coroutine = MsgraphEventUpdate::new(&auth(), "me", "EV2", &event).unwrap();
    let (result, written) = run(
        &mut coroutine,
        &json_response("HTTP/1.1 200 OK", r#"{ "id": "EV2" }"#),
    );
    assert!(result.is_ok());

    let request = String::from_utf8_lossy(&written);
    assert!(
        request.starts_with("PATCH /v1.0/me/events/EV2"),
        "got: {request}"
    );
    assert!(
        request.ends_with(r#"{"subject":"Lunch","location":null}"#),
        "got: {request}"
    );
}

#[test]
fn event_instances_and_calendar_view_carry_their_window() {
    use io_msgraph::v1::rest::users::events::{
        calendar_view::MsgraphCalendarView, instances::MsgraphEventInstances,
        list::MsgraphEventsListParams,
    };

    let params = MsgraphEventsListParams::default();
    let empty = json_response("HTTP/1.1 200 OK", r#"{ "value": [] }"#);

    let mut coroutine = MsgraphEventInstances::new(
        &auth(),
        "me",
        "EV1",
        "2026-01-01T00:00:00Z",
        "2027-01-01T00:00:00Z",
        &params,
    )
    .unwrap();
    let (result, written) = run(&mut coroutine, &empty);
    assert!(result.is_ok());
    let request = String::from_utf8_lossy(&written);
    assert!(
        request.starts_with("GET /v1.0/me/events/EV1/instances?"),
        "got: {request}"
    );
    assert!(
        request.contains("startDateTime=2026-01-01T00%3A00%3A00Z"),
        "got: {request}"
    );
    assert!(
        request.contains("endDateTime=2027-01-01T00%3A00%3A00Z"),
        "got: {request}"
    );

    let mut coroutine = MsgraphCalendarView::new(
        &auth(),
        "me",
        Some("CAL1"),
        "2026-01-01T00:00:00Z",
        "2027-01-01T00:00:00Z",
        &params,
    )
    .unwrap();
    let (result, written) = run(&mut coroutine, &empty);
    assert!(result.is_ok());
    let request = String::from_utf8_lossy(&written);
    assert!(
        request.starts_with("GET /v1.0/me/calendars/CAL1/calendarView?"),
        "got: {request}"
    );
}

#[test]
fn events_delta_parses_changes_and_removals() {
    use io_msgraph::v1::rest::users::events::delta::MsgraphEventsDelta;

    let body = r#"{
        "value": [
            { "id": "EV1", "seriesMasterId": "SM1", "type": "exception", "changeKey": "ck2" },
            { "id": "EV3", "@removed": { "reason": "deleted" } }
        ],
        "@odata.deltaLink": "https://graph.microsoft.com/v1.0/me/calendarView/delta?$deltatoken=abc"
    }"#;
    let mut coroutine = MsgraphEventsDelta::new(
        &auth(),
        "me",
        None,
        "2026-01-01T00:00:00Z",
        "2027-01-01T00:00:00Z",
    )
    .unwrap();
    let (result, written) = run(&mut coroutine, &json_response("HTTP/1.1 200 OK", body));
    let out = result.unwrap().response;

    let request = String::from_utf8_lossy(&written);
    assert!(
        request.starts_with("GET /v1.0/me/calendarView/delta?"),
        "got: {request}"
    );
    assert_eq!(out.value[0].event.series_master_id.as_deref(), Some("SM1"));
    assert!(out.value[0].removed.is_none());
    assert_eq!(out.value[1].removed.as_ref().unwrap().reason, "deleted");
    assert!(out.delta_link.unwrap().contains("deltatoken=abc"));
}

#[test]
fn todo_lists_list_reads_the_built_in_lists() {
    use io_msgraph::v1::rest::users::todo::lists::{
        MsgraphTodoWellknownListName,
        list::{MsgraphTodoListsList, MsgraphTodoListsListParams},
    };

    let body = r#"{
        "value": [
            { "@odata.etag": "W/\"vpgg2g==\"", "displayName": "Tasks", "isOwner": true,
              "isShared": false, "wellknownListName": "defaultList", "id": "TL1" },
            { "displayName": "Later", "wellknownListName": "somethingNew", "id": "TL2" }
        ]
    }"#;
    let params = MsgraphTodoListsListParams::default();
    let mut coroutine = MsgraphTodoListsList::new(&auth(), "alice@example.com", &params).unwrap();
    let (result, written) = run(&mut coroutine, &json_response("HTTP/1.1 200 OK", body));
    let out = result.unwrap().response;

    let request = String::from_utf8_lossy(&written);
    assert!(
        request.starts_with("GET /v1.0/users/alice@example.com/todo/lists?"),
        "got: {request}"
    );
    assert_eq!(out.value[0].etag.as_deref(), Some("W/\"vpgg2g==\""));
    assert_eq!(
        out.value[0].wellknown_list_name,
        Some(MsgraphTodoWellknownListName::DefaultList)
    );
    assert_eq!(
        out.value[1].wellknown_list_name,
        Some(MsgraphTodoWellknownListName::UnknownFutureValue)
    );
}

#[test]
fn todo_task_get_expands_into_its_list() {
    use io_msgraph::v1::rest::users::todo::tasks::get::MsgraphTodoTaskGet;

    let body = r#"{
        "id": "TK1",
        "title": "Call",
        "checklistItems": [{ "displayName": "Dial", "isChecked": false, "id": "CI1" }],
        "linkedResources": [{ "webUrl": "mid:abc@example.com", "id": "LR1" }]
    }"#;
    let mut coroutine =
        MsgraphTodoTaskGet::new(&auth(), "me", "TL1", "TK1", None, Some("checklistItems")).unwrap();
    let (result, written) = run(&mut coroutine, &json_response("HTTP/1.1 200 OK", body));
    let out = result.unwrap().response;

    let request = String::from_utf8_lossy(&written);
    assert!(
        request.starts_with("GET /v1.0/me/todo/lists/TL1/tasks/TK1?%24expand=checklistItems "),
        "got: {request}"
    );
    assert_eq!(out.checklist_items[0].display_name.as_deref(), Some("Dial"));
    assert_eq!(
        out.linked_resources[0].web_url.as_deref(),
        Some("mid:abc@example.com")
    );
}

#[test]
fn todo_task_writes_post_and_patch_into_the_list() {
    use io_msgraph::v1::rest::users::todo::tasks::{
        MsgraphTodoTask, create::MsgraphTodoTaskCreate, update::MsgraphTodoTaskUpdate,
    };

    let read: MsgraphTodoTask = serde_json::from_str(
        r#"{ "@odata.etag": "W/\"e1\"", "id": "TK1", "title": "Call", "hasAttachments": false }"#,
    )
    .unwrap();
    let task = MsgraphTodoTask {
        title: MsgraphField::Set("Call".into()),
        ..Default::default()
    };

    let mut coroutine = MsgraphTodoTaskCreate::new(&auth(), "me", "TL1", &task).unwrap();
    let (result, written) = run(
        &mut coroutine,
        &json_response("HTTP/1.1 201 Created", r#"{ "id": "TK1" }"#),
    );
    assert!(result.is_ok());
    let request = String::from_utf8_lossy(&written);
    assert!(
        request.starts_with("POST /v1.0/me/todo/lists/TL1/tasks "),
        "got: {request}"
    );
    assert!(request.ends_with(r#"{"title":"Call"}"#), "got: {request}");

    // NOTE: a read task written back carries its etag; the edit clears the
    // due date.
    let task = MsgraphTodoTask {
        etag: None,
        due_date_time: MsgraphField::Null,
        ..read
    };
    let mut coroutine = MsgraphTodoTaskUpdate::new(&auth(), "me", "TL1", "TK1", &task).unwrap();
    let (result, written) = run(
        &mut coroutine,
        &json_response("HTTP/1.1 200 OK", r#"{ "id": "TK1" }"#),
    );
    assert!(result.is_ok());
    let request = String::from_utf8_lossy(&written);
    assert!(
        request.starts_with("PATCH /v1.0/me/todo/lists/TL1/tasks/TK1 "),
        "got: {request}"
    );
    assert!(
        request
            .ends_with(r#"{"id":"TK1","title":"Call","dueDateTime":null,"hasAttachments":false}"#),
        "got: {request}"
    );
}

#[test]
fn todo_tasks_delta_parses_changes_and_removals() {
    use io_msgraph::v1::rest::users::todo::tasks::{
        MsgraphTodoTaskStatus, delta::MsgraphTodoTasksDelta,
    };

    // NOTE: a removal row as Graph sends it, with two defaulted fields.
    let body = r#"{
        "value": [
            { "isReminderOn": false, "hasAttachments": false, "id": "TK2",
              "@removed": { "reason": "deleted" } },
            { "@odata.etag": "W/\"e2\"", "status": "waitingOnOthers", "title": "Call",
              "id": "TK1", "categories": ["inbox"],
              "linkedResources": [{ "webUrl": "mid:abc@example.com", "id": "LR1" }] }
        ],
        "@odata.deltaLink": "https://graph.microsoft.com/v1.0/me/todo/lists/TL1/tasks/delta?$deltatoken=abc"
    }"#;
    let mut coroutine = MsgraphTodoTasksDelta::new(&auth(), "me", "TL1", None)
        .unwrap()
        .max_page_size(50);
    let (result, written) = run(&mut coroutine, &json_response("HTTP/1.1 200 OK", body));
    let out = result.unwrap().response;

    let request = String::from_utf8_lossy(&written);
    assert!(
        request.starts_with("GET /v1.0/me/todo/lists/TL1/tasks/delta "),
        "got: {request}"
    );
    assert!(
        request.contains("Prefer: odata.maxpagesize=50"),
        "got: {request}"
    );
    assert_eq!(out.value[0].task.id, "TK2");
    assert_eq!(out.value[0].removed.as_ref().unwrap().reason, "deleted");
    assert!(out.value[1].removed.is_none());
    assert_eq!(out.value[1].task.etag.as_deref(), Some("W/\"e2\""));
    assert_eq!(
        out.value[1].task.status,
        MsgraphField::Set(MsgraphTodoTaskStatus::WaitingOnOthers)
    );
    assert!(out.delta_link.unwrap().contains("deltatoken=abc"));
}
