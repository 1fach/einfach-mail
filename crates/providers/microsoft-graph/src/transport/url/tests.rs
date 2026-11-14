use einfach_mail_domain::CalendarViewRange;
use time::{Date, Month};
use url::Url;

use super::{CALENDAR_EVENT_EDIT_SELECT, GraphAuthority, MESSAGE_SELECT, MailFolderKind};
use crate::error::GraphError;

fn authority() -> GraphAuthority {
    GraphAuthority::from_profile_endpoint(
        &Url::parse("https://graph.microsoft.com/v1.0/me?select=id")
            .expect("synthetic Graph endpoint is valid"),
    )
    .expect("synthetic Graph authority is valid")
}

#[test]
fn rejects_links_outside_the_selected_graph_origin_and_path() {
    for link in [
        "https://example.test/v1.0/me",
        "https://graph.microsoft.com/v1.0.evil/me",
        "https://graph.microsoft.com/v1.0/me#fragment",
        "https://user@graph.microsoft.com/v1.0/me",
        "http://graph.microsoft.com/v1.0/me",
    ] {
        assert!(matches!(
            authority().opaque_link(link),
            Err(GraphError::UnsafeLink)
        ));
    }
}

#[test]
fn calendar_delta_query_contains_only_the_exact_utc_view_bounds() {
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
    let request = authority()
        .calendar_view_delta_path(&range)
        .expect("synthetic calendar delta URL is valid");
    let url = Url::parse(request.as_str()).expect("synthetic Graph URL is valid");
    let query = url.query_pairs().collect::<Vec<_>>();

    assert_eq!(url.path(), "/v1.0/me/calendarView/delta");
    assert_eq!(
        query,
        [
            ("startDateTime".into(), "2030-01-07T00:00:00Z".into()),
            ("endDateTime".into(), "2030-01-14T00:00:00Z".into()),
        ]
    );
}

#[test]
fn primary_calendar_event_path_targets_only_the_signed_in_users_default_calendar() {
    let request = authority()
        .primary_calendar_events_path()
        .expect("synthetic primary calendar event URL is valid");
    let url = Url::parse(request.as_str()).expect("synthetic Graph URL is valid");

    assert_eq!(url.path(), "/v1.0/me/calendar/events");
    assert!(url.query().is_none());
}

#[test]
fn calendar_event_path_escapes_opaque_ids_without_query_options() {
    let request = authority()
        .calendar_event_path("event/id+1")
        .expect("synthetic event ID is safely encoded");
    let url = Url::parse(request.as_str()).expect("synthetic Graph URL is valid");

    assert_eq!(url.path(), "/v1.0/me/events/event%2Fid+1");
    assert!(url.query().is_none());
}

#[test]
fn versioned_calendar_event_path_selects_only_edit_data() {
    let request = authority()
        .versioned_calendar_event_path("event/id+1")
        .expect("synthetic event ID is safely encoded");
    let url = Url::parse(request.as_str()).expect("synthetic Graph URL is valid");

    assert_eq!(url.path(), "/v1.0/me/events/event%2Fid+1");
    assert_eq!(
        url.query_pairs().collect::<Vec<_>>(),
        [("$select".into(), CALENDAR_EVENT_EDIT_SELECT.into())]
    );
}

#[test]
fn accepts_only_bounded_reset_links_for_the_exact_calendar_range() {
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
    let valid_link = "https://graph.microsoft.com/v1.0/me/calendarView/delta?startDateTime=2030-01-07T00%3A00%3A00Z&endDateTime=2030-01-14T00%3A00%3A00Z&$deltatoken=";
    let authority = authority();

    assert_eq!(
        authority
            .calendar_delta_reset_link(&range, valid_link)
            .expect("reset link matches the requested view")
            .as_str(),
        valid_link
    );
    for link in [
        "https://example.test/v1.0/me/calendarView/delta?startDateTime=2030-01-07T00%3A00%3A00Z&endDateTime=2030-01-14T00%3A00%3A00Z&$deltatoken=",
        "https://graph.microsoft.com/v1.0/me/calendarView/delta?startDateTime=2030-01-07T00%3A00%3A00Z&endDateTime=2030-01-21T00%3A00%3A00Z&$deltatoken=",
        "https://graph.microsoft.com/v1.0/me/calendarView/delta?startDateTime=2030-01-07T00%3A00%3A00Z&endDateTime=2030-01-14T00%3A00%3A00Z",
        "https://graph.microsoft.com/v1.0/me/calendarView/delta?startDateTime=2030-01-07T00%3A00%3A00Z&endDateTime=2030-01-14T00%3A00%3A00Z&$deltatoken=expired",
        "https://graph.microsoft.com/v1.0/me/calendarView/delta?startDateTime=2030-01-07T00%3A00%3A00Z&startDateTime=2030-01-07T00%3A00%3A00Z&endDateTime=2030-01-14T00%3A00%3A00Z&$deltatoken=",
        "https://graph.microsoft.com/v1.0/me/calendarView/delta?startDateTime=2030-01-07T00%3A00%3A00Z&endDateTime=2030-01-14T00%3A00%3A00Z&$deltatoken=&$deltatoken=",
        "https://graph.microsoft.com/v1.0/me/calendarView/delta?startDateTime=2030-01-07T00%3A00%3A00Z&endDateTime=2030-01-14T00%3A00%3A00Z&$deltatoken=&$top=50",
        "https://graph.microsoft.com/v1.0/me/calendarView/delta?startDateTime=2030-01-07T00%3A00%3A00Z%2&endDateTime=2030-01-14T00%3A00%3A00Z&$deltatoken=",
        "https://graph.microsoft.com/v1.0/me/calendarView/delta?startDateTime=2030-01-07T00%3A00%3A00Z&endDateTime=2030-01-14T00%3A00%3A00Z&$deltatoken",
        "https://graph.microsoft.com/v1.0/me/events/delta?startDateTime=2030-01-07T00%3A00%3A00Z&endDateTime=2030-01-14T00%3A00%3A00Z&$deltatoken=",
    ] {
        assert_eq!(
            authority.calendar_delta_reset_link(&range, link).err(),
            Some(GraphError::UnsafeLink)
        );
    }

    let oversized_link = format!(
        "{valid_link}{}",
        "x".repeat(super::MAX_GRAPH_CALENDAR_DELTA_URL_BYTES)
    );
    assert_eq!(
        authority
            .calendar_delta_reset_link(&range, &oversized_link)
            .err(),
        Some(GraphError::ResponseTooLarge)
    );
}

#[test]
fn retains_valid_opaque_links_without_normalizing_them() {
    let link = "https://graph.microsoft.com/v1.0/me/messages?$skiptoken=A%2FB%3D";

    assert_eq!(
        authority()
            .opaque_link(link)
            .expect("opaque link has the configured authority")
            .as_str(),
        link
    );
}

#[test]
fn message_reply_path_encodes_source_id_as_one_segment() {
    let request = authority()
        .message_reply_path("message/id with space")
        .expect("synthetic source ID forms a safe reply URL");

    assert!(
        request
            .as_str()
            .contains("/me/messages/message%2Fid%20with%20space/reply")
    );
    assert!(
        Url::parse(request.as_str())
            .expect("synthetic reply URL is valid")
            .query()
            .is_none()
    );
}

#[test]
fn permits_only_loopback_http_for_synthetic_graph_authorities() {
    let local =
        Url::parse("http://127.0.0.1:45879/v1.0/me").expect("synthetic loopback endpoint is valid");
    let public_http =
        Url::parse("http://graph.microsoft.com/v1.0/me").expect("synthetic HTTP endpoint is valid");

    assert!(GraphAuthority::from_profile_endpoint(&local).is_ok());
    assert_eq!(
        GraphAuthority::from_profile_endpoint(&public_http).err(),
        Some(GraphError::UnsafeLink)
    );
}

#[test]
fn constructs_the_me_send_mail_endpoint() {
    assert_eq!(
        authority()
            .mail_send_path()
            .expect("send endpoint uses the selected Graph authority")
            .as_str(),
        "https://graph.microsoft.com/v1.0/me/sendMail"
    );
}

#[test]
fn folder_delta_ids_are_encoded_as_single_path_segments() {
    let url = authority()
        .folder_message_delta_path("folder/id with space")
        .expect("folder ID forms an encoded Graph delta path");

    assert!(url.as_str().contains("folder%2Fid%20with%20space"));
}

#[test]
fn well_known_paths_use_locale_independent_folder_names() {
    for (folder, expected) in [
        (
            MailFolderKind::Inbox,
            "https://graph.microsoft.com/v1.0/me/mailFolders/inbox?%24select=id%2CdisplayName%2CisHidden",
        ),
        (
            MailFolderKind::SentItems,
            "https://graph.microsoft.com/v1.0/me/mailFolders/sentitems?%24select=id%2CdisplayName%2CisHidden",
        ),
        (
            MailFolderKind::Drafts,
            "https://graph.microsoft.com/v1.0/me/mailFolders/drafts?%24select=id%2CdisplayName%2CisHidden",
        ),
        (
            MailFolderKind::Archive,
            "https://graph.microsoft.com/v1.0/me/mailFolders/archive?%24select=id%2CdisplayName%2CisHidden",
        ),
        (
            MailFolderKind::DeletedItems,
            "https://graph.microsoft.com/v1.0/me/mailFolders/deleteditems?%24select=id%2CdisplayName%2CisHidden",
        ),
        (
            MailFolderKind::JunkEmail,
            "https://graph.microsoft.com/v1.0/me/mailFolders/junkemail?%24select=id%2CdisplayName%2CisHidden",
        ),
    ] {
        assert_eq!(
            authority()
                .well_known_folder_path(folder)
                .expect("well-known folder endpoint is a valid Graph URL")
                .as_str(),
            expected
        );
    }
}

#[test]
fn message_delta_and_body_ids_are_encoded_as_path_segments() {
    let authority = authority();
    let delta_url = authority
        .folder_message_delta_path("folder/id with space")
        .expect("folder ID forms an encoded Graph delta path");
    let body_url = authority
        .message_body_path("message/id with space")
        .expect("message ID forms an encoded Graph body path");
    let read_state_url = authority
        .message_update_path("message/id with space")
        .expect("message ID forms an encoded Graph update path");

    assert!(
        delta_url
            .as_str()
            .contains("folder%2Fid%20with%20space/messages/delta")
    );
    assert!(body_url.as_str().contains("message%2Fid%20with%20space"));
    assert!(
        read_state_url
            .as_str()
            .contains("message%2Fid%20with%20space")
    );
    assert!(
        Url::parse(read_state_url.as_str())
            .expect("synthetic Graph update URL is valid")
            .query()
            .is_none()
    );
    let body_query = Url::parse(body_url.as_str())
        .expect("synthetic Graph body URL is valid")
        .query_pairs()
        .find(|(name, _)| name == "$select")
        .map(|(_, value)| value.into_owned());
    assert_eq!(body_query.as_deref(), Some("body"));
}

#[test]
fn accepts_only_reset_links_for_the_requested_folder_delta_path() {
    let authority = authority();
    let link = format!(
        "https://graph.microsoft.com/v1.0/me/mailFolders/folder/messages/delta?$deltatoken=&$select={MESSAGE_SELECT}"
    );

    assert_eq!(
        authority
            .message_delta_reset_link("folder", &link)
            .expect("reset link matches the requested folder query")
            .as_str(),
        link
    );
    for link in [
        "https://example.test/v1.0/me/mailFolders/folder/messages/delta?$deltatoken=&$select=id,subject,from,sender,receivedDateTime,isRead",
        "https://graph.microsoft.com/v1.0/me/mailFolders/other/messages/delta?$deltatoken=&$select=id,subject,from,sender,receivedDateTime,isRead",
        "https://graph.microsoft.com/v1.0/me/messages/delta?$deltatoken=&$select=id,subject,from,sender,receivedDateTime,isRead",
        "https://graph.microsoft.com/v1.0/me/mailFolders/folder/messages/delta?$deltatoken=expired&$select=id,subject,from,sender,receivedDateTime,isRead",
        "https://graph.microsoft.com/v1.0/me/mailFolders/folder/messages/delta?$deltatoken=&$deltatoken=&$select=id,subject,from,sender,receivedDateTime,isRead",
        "https://graph.microsoft.com/v1.0/me/mailFolders/folder/messages/delta?$deltatoken=&$select=id",
        "https://graph.microsoft.com/v1.0/me/mailFolders/folder/messages/delta?$deltatoken=&$select=id,subject,from,sender,receivedDateTime,isRead&$top=50",
    ] {
        assert_eq!(
            authority.message_delta_reset_link("folder", link).err(),
            Some(GraphError::UnsafeLink)
        );
    }
}
