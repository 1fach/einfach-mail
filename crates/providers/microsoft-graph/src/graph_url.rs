#[cfg(test)]
use std::net::IpAddr;

use time::format_description::well_known::Rfc3339;
use url::Url;

use crate::error::GraphError;
use einfach_mail_application::WellKnownMailFolder;
use einfach_mail_domain::CalendarViewRange;

const GRAPH_API_PATH_PREFIX: &str = "/v1.0/";
const FOLDER_SELECT: &str = "id,displayName,isHidden";
const MESSAGE_SELECT: &str = "id,subject,from,sender,receivedDateTime,isRead,flag";
pub(super) const CALENDAR_EVENT_EDIT_SELECT: &str = "id,subject,location,start,end,isAllDay,originalStartTimeZone,originalEndTimeZone,type,isOrganizer,attendees,isOnlineMeeting,changeKey";

#[derive(Clone)]
pub(super) struct GraphAuthority {
    base_url: Url,
    origin: GraphOrigin,
}

#[derive(Clone, Eq, PartialEq)]
struct GraphOrigin {
    scheme: String,
    host: String,
    port: u16,
}

impl GraphAuthority {
    pub(super) fn from_profile_endpoint(endpoint: &Url) -> Result<Self, GraphError> {
        if endpoint.path() != "/v1.0/me" {
            return Err(GraphError::UnsafeLink);
        }
        let mut base_url = endpoint.clone();
        base_url.set_path(GRAPH_API_PATH_PREFIX);
        base_url.set_query(None);
        base_url.set_fragment(None);
        Self::from_base_url(base_url)
    }

    pub(super) fn profile_url(&self, endpoint: &Url) -> Result<GraphUrl, GraphError> {
        self.opaque_link(endpoint.as_str())
    }

    pub(super) fn root_folders_path(&self) -> Result<GraphUrl, GraphError> {
        let mut url = self
            .base_url
            .join("me/mailFolders")
            .map_err(|_| GraphError::UnsafeLink)?;
        url.query_pairs_mut().append_pair("$select", FOLDER_SELECT);
        self.validated_url(url)
    }

    pub(super) fn well_known_folder_path(
        &self,
        folder: WellKnownMailFolder,
    ) -> Result<GraphUrl, GraphError> {
        let folder_name = match folder {
            WellKnownMailFolder::Archive => "archive",
            WellKnownMailFolder::DeletedItems => "deleteditems",
        };
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .map_err(|()| GraphError::UnsafeLink)?
            .pop_if_empty()
            .extend(["me", "mailFolders", folder_name]);
        url.query_pairs_mut().append_pair("$select", FOLDER_SELECT);
        self.validated_url(url)
    }

    pub(super) fn child_folders_path(&self, folder_id: &str) -> Result<GraphUrl, GraphError> {
        let mut url = self.folder_path(folder_id, "childFolders")?;
        url.query_pairs_mut().append_pair("$select", FOLDER_SELECT);
        self.validated_url(url)
    }

    pub(super) fn folder_messages_path(&self, folder_id: &str) -> Result<GraphUrl, GraphError> {
        let mut url = self.folder_path(folder_id, "messages")?;
        url.query_pairs_mut().append_pair("$select", MESSAGE_SELECT);
        self.validated_url(url)
    }

    pub(super) fn folder_message_delta_path(
        &self,
        folder_id: &str,
    ) -> Result<GraphUrl, GraphError> {
        let mut url = self.folder_path(folder_id, "messages")?;
        url.path_segments_mut()
            .map_err(|()| GraphError::UnsafeLink)?
            .push("delta");
        url.query_pairs_mut().append_pair("$select", MESSAGE_SELECT);
        self.validated_url(url)
    }

    pub(super) fn message_body_path(&self, message_id: &str) -> Result<GraphUrl, GraphError> {
        let mut url = self.message_path(message_id)?;
        url.query_pairs_mut().append_pair("$select", "body");
        self.validated_url(url)
    }

    pub(super) fn message_update_path(&self, message_id: &str) -> Result<GraphUrl, GraphError> {
        self.validated_url(self.message_path(message_id)?)
    }

    pub(super) fn message_move_path(&self, message_id: &str) -> Result<GraphUrl, GraphError> {
        let mut url = self.message_path(message_id)?;
        url.path_segments_mut()
            .map_err(|()| GraphError::UnsafeLink)?
            .push("move");
        self.validated_url(url)
    }

    pub(super) fn message_reply_path(&self, message_id: &str) -> Result<GraphUrl, GraphError> {
        let mut url = self.message_path(message_id)?;
        url.path_segments_mut()
            .map_err(|()| GraphError::UnsafeLink)?
            .push("reply");
        self.validated_url(url)
    }

    pub(super) fn mail_send_path(&self) -> Result<GraphUrl, GraphError> {
        let url = self
            .base_url
            .join("me/sendMail")
            .map_err(|_| GraphError::UnsafeLink)?;
        self.validated_url(url)
    }

    pub(super) fn calendar_view_delta_path(
        &self,
        range: &CalendarViewRange,
    ) -> Result<GraphUrl, GraphError> {
        let mut url = self
            .base_url
            .join("me/calendarView/delta")
            .map_err(|_| GraphError::UnsafeLink)?;
        let start = range
            .start()
            .format(&Rfc3339)
            .map_err(|_| GraphError::InvalidRecord)?;
        let end = range
            .end()
            .format(&Rfc3339)
            .map_err(|_| GraphError::InvalidRecord)?;
        url.query_pairs_mut()
            .append_pair("startDateTime", &start)
            .append_pair("endDateTime", &end);
        self.validated_url(url)
    }

    pub(super) fn primary_calendar_events_path(&self) -> Result<GraphUrl, GraphError> {
        let url = self
            .base_url
            .join("me/calendar/events")
            .map_err(|_| GraphError::UnsafeLink)?;
        self.validated_url(url)
    }

    pub(super) fn calendar_event_path(&self, event_id: &str) -> Result<GraphUrl, GraphError> {
        self.validated_url(self.calendar_event_url(event_id)?)
    }

    pub(super) fn editable_calendar_event_path(
        &self,
        event_id: &str,
    ) -> Result<GraphUrl, GraphError> {
        let mut url = self.calendar_event_url(event_id)?;
        url.query_pairs_mut()
            .append_pair("$select", CALENDAR_EVENT_EDIT_SELECT);
        self.validated_url(url)
    }

    fn calendar_event_url(&self, event_id: &str) -> Result<Url, GraphError> {
        if event_id.is_empty() {
            return Err(GraphError::InvalidRecord);
        }
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .map_err(|()| GraphError::UnsafeLink)?
            .pop_if_empty()
            .extend(["me", "events", event_id]);
        Ok(url)
    }

    pub(super) fn opaque_link(&self, link: &str) -> Result<GraphUrl, GraphError> {
        if has_user_info_separator(link) {
            return Err(GraphError::UnsafeLink);
        }
        let url = Url::parse(link).map_err(|_| GraphError::UnsafeLink)?;
        self.validate_url(&url)?;
        Ok(GraphUrl(link.to_owned()))
    }

    pub(super) fn message_delta_reset_link(
        &self,
        folder_id: &str,
        location: &str,
    ) -> Result<GraphUrl, GraphError> {
        let reset_link = self.opaque_link(location)?;
        let expected_path = self.folder_message_delta_path(folder_id)?;
        let reset_url = Url::parse(reset_link.as_str()).map_err(|_| GraphError::UnsafeLink)?;
        let expected_url =
            Url::parse(expected_path.as_str()).map_err(|_| GraphError::UnsafeLink)?;
        let mut has_empty_delta_token = false;
        let mut has_message_select = false;
        for (name, value) in reset_url.query_pairs() {
            match name.as_ref() {
                "$deltatoken" if !has_empty_delta_token && value.is_empty() => {
                    has_empty_delta_token = true;
                }
                "$select" if !has_message_select && value == MESSAGE_SELECT => {
                    has_message_select = true;
                }
                _ => return Err(GraphError::UnsafeLink),
            }
        }
        if reset_url.path() != expected_url.path() || !has_empty_delta_token || !has_message_select
        {
            return Err(GraphError::UnsafeLink);
        }
        Ok(reset_link)
    }

    fn folder_path(&self, folder_id: &str, tail: &str) -> Result<Url, GraphError> {
        if folder_id.is_empty() {
            return Err(GraphError::InvalidRecord);
        }
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .map_err(|()| GraphError::UnsafeLink)?
            .pop_if_empty()
            .extend(["me", "mailFolders", folder_id, tail]);
        Ok(url)
    }

    fn message_path(&self, message_id: &str) -> Result<Url, GraphError> {
        if message_id.is_empty() {
            return Err(GraphError::InvalidRecord);
        }
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .map_err(|()| GraphError::UnsafeLink)?
            .pop_if_empty()
            .extend(["me", "messages", message_id]);
        Ok(url)
    }

    fn from_base_url(base_url: Url) -> Result<Self, GraphError> {
        if !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
            || base_url.path() != GRAPH_API_PATH_PREFIX
            || !is_allowed_graph_origin(&base_url)
        {
            return Err(GraphError::UnsafeLink);
        }
        let origin = GraphOrigin::from_url(&base_url)?;
        Ok(Self { base_url, origin })
    }

    fn validate_url(&self, url: &Url) -> Result<(), GraphError> {
        if !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || !url.path().starts_with(GRAPH_API_PATH_PREFIX)
            || !is_allowed_graph_origin(url)
            || GraphOrigin::from_url(url)? != self.origin
        {
            return Err(GraphError::UnsafeLink);
        }
        Ok(())
    }

    fn validated_url(&self, url: Url) -> Result<GraphUrl, GraphError> {
        self.validate_url(&url)?;
        Ok(GraphUrl(url.to_string()))
    }
}

impl GraphOrigin {
    fn from_url(url: &Url) -> Result<Self, GraphError> {
        Ok(Self {
            scheme: url.scheme().to_owned(),
            host: url.host_str().ok_or(GraphError::UnsafeLink)?.to_owned(),
            port: url.port_or_known_default().ok_or(GraphError::UnsafeLink)?,
        })
    }
}

fn is_allowed_graph_origin(url: &Url) -> bool {
    let global_graph = url.scheme() == "https"
        && url.host_str() == Some("graph.microsoft.com")
        && url.port_or_known_default() == Some(443);
    #[cfg(test)]
    let loopback_fixture = url.scheme() == "http"
        && url
            .host_str()
            .and_then(|host| host.parse::<IpAddr>().ok())
            .is_some_and(|address| address.is_loopback())
        && url.port_or_known_default().is_some();
    #[cfg(not(test))]
    let loopback_fixture = false;
    global_graph || loopback_fixture
}

fn has_user_info_separator(raw_url: &str) -> bool {
    raw_url
        .split_once("://")
        .is_some_and(|(_, authority_and_rest)| {
            authority_and_rest
                .split(['/', '?', '#'])
                .next()
                .is_some_and(|authority| authority.contains('@'))
        })
}

#[derive(Clone)]
pub(super) struct GraphUrl(String);

impl GraphUrl {
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }

    #[cfg(test)]
    pub(super) fn fixture(url: Url) -> Self {
        Self(url.to_string())
    }
}

#[cfg(test)]
mod tests {
    use einfach_mail_domain::CalendarViewRange;
    use time::{Date, Month};
    use url::Url;

    use super::{CALENDAR_EVENT_EDIT_SELECT, GraphAuthority, MESSAGE_SELECT, WellKnownMailFolder};
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
    fn editable_calendar_event_path_selects_only_edit_data() {
        let request = authority()
            .editable_calendar_event_path("event/id+1")
            .expect("synthetic event ID is safely encoded");
        let url = Url::parse(request.as_str()).expect("synthetic Graph URL is valid");

        assert_eq!(url.path(), "/v1.0/me/events/event%2Fid+1");
        assert_eq!(
            url.query_pairs().collect::<Vec<_>>(),
            [("$select".into(), CALENDAR_EVENT_EDIT_SELECT.into())]
        );
    }

    #[test]
    fn mail_send_path_targets_the_signed_in_users_mailbox() {
        let request = authority()
            .mail_send_path()
            .expect("synthetic send mail URL is valid");
        let url = Url::parse(request.as_str()).expect("synthetic Graph URL is valid");

        assert_eq!(url.path(), "/v1.0/me/sendMail");
        assert!(url.query().is_none());
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
        let local = Url::parse("http://127.0.0.1:45879/v1.0/me")
            .expect("synthetic loopback endpoint is valid");
        let public_http = Url::parse("http://graph.microsoft.com/v1.0/me")
            .expect("synthetic HTTP endpoint is valid");

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
    fn folder_ids_are_encoded_as_single_path_segments() {
        let authority = authority();
        let url = authority
            .folder_messages_path("folder/id with space")
            .expect("folder ID forms an encoded Graph path");

        assert!(url.as_str().contains("folder%2Fid%20with%20space"));
    }

    #[test]
    fn well_known_paths_use_locale_independent_folder_names() {
        for (folder, expected) in [
            (
                WellKnownMailFolder::Archive,
                "https://graph.microsoft.com/v1.0/me/mailFolders/archive?%24select=id%2CdisplayName%2CisHidden",
            ),
            (
                WellKnownMailFolder::DeletedItems,
                "https://graph.microsoft.com/v1.0/me/mailFolders/deleteditems?%24select=id%2CdisplayName%2CisHidden",
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
}
