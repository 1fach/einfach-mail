use std::{sync::Arc, time::SystemTime};

use httpmock::{
    Method::{DELETE, GET, PATCH, POST},
    MockServer,
};
use huskarl::token::AccessToken;
use serde_json::json;
use time::{
    Date, Month, OffsetDateTime, PrimitiveDateTime, format_description::well_known::Rfc3339,
};
use url::Url;

use einfach_mail_application::{
    CalendarDeltaRequest, CalendarDeltaResponse, MessageDeltaRequest, MessageDeltaResponse,
    WellKnownMailFolder,
};
use einfach_mail_domain::{
    AccountId, AccountProfile, CalendarCapabilities, CalendarDeltaChange, CalendarDeltaLink,
    CalendarDeltaProgress, CalendarEventCreation, CalendarEventDraft, CalendarEventUpdate,
    CalendarEventUpdateDraft, CalendarOccurrence, CalendarOccurrenceTime, CalendarTimeZoneId,
    CalendarViewRange, CalendarViewReference, EditableCalendarEvent, FolderId, FolderReference,
    FollowUpFlagStatus, MailDraft, MailRecipient, MailReplyDraft, MailReplySource, MailSubject,
    MessageDeltaChange, MessageId, MessageMetadata, MessageOrigin, ReadState,
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
use crate::test_support::{
    http_base_url, http_url, synthetic_access_token as make_synthetic_access_token,
};
use crate::{
    error::GraphError,
    transport::{CALENDAR_EVENT_EDIT_SELECT, GraphAuthority},
    transport::{GraphTransport, GraphTransportHandle},
};

fn synthetic_access_token() -> Arc<AccessToken> {
    make_synthetic_access_token("synthetic-calendar-access-token")
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

fn synthetic_calendar_creation() -> CalendarEventCreation {
    let start = synthetic_utc_datetime("2030-01-08T09:30:00Z");
    let end = synthetic_utc_datetime("2030-01-08T10:00:00Z");
    let draft = CalendarEventDraft::timed(
        "Synthetic planning meeting".to_owned(),
        PrimitiveDateTime::new(start.date(), start.time()),
        PrimitiveDateTime::new(end.date(), end.time()),
        CalendarTimeZoneId::try_from("Etc/UTC".to_owned()).expect("synthetic zone is valid"),
        start,
        end,
    )
    .expect("synthetic event draft is valid");

    CalendarEventCreation::try_new(synthetic_view(), draft)
        .expect("synthetic event fits its calendar view")
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
    let endpoint = Url::parse(&http_url(server, "/v1.0/me"))
        .expect("synthetic Graph profile endpoint is valid");
    let authority = GraphAuthority::from_profile_endpoint(&endpoint)
        .expect("synthetic Graph authority is valid");
    let transport =
        GraphTransport::start_for_fixtures().expect("synthetic Graph transport worker starts");
    let handle: GraphTransportHandle = transport.handle();
    (GraphApi::new(authority, handle), transport)
}

async fn assert_create_event_response_is_unknown(
    creation: &CalendarEventCreation,
    response: serde_json::Value,
) {
    let server = MockServer::start_async().await;
    let create = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/v1.0/me/calendar/events")
                .body_includes(format!("\"transactionId\":\"{}\"", creation.id().as_uuid()));
            then.status(201)
                .header("content-type", "application/json")
                .body(response.to_string());
        })
        .await;
    let (api, _transport) = synthetic_graph_api(&server);
    assert!(matches!(
        api.create_calendar_event(creation, synthetic_access_token())
            .await,
        Err(GraphError::CalendarEventCreationOutcomeUnknown)
    ));
    create.assert_async().await;
}

include!("tests/account.rs");
include!("tests/folders.rs");
include!("tests/pagination.rs");
include!("tests/mail.rs");
include!("tests/calendar.rs");
