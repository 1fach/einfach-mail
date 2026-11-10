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
pub const CALENDAR_EVENT_EDIT_SELECT: &str = "id,subject,location,start,end,isAllDay,originalStartTimeZone,originalEndTimeZone,type,isOrganizer,attendees,isOnlineMeeting,changeKey";
pub const MAX_GRAPH_CALENDAR_DELTA_URL_BYTES: usize = 16 * 1024;

#[derive(Clone)]
pub struct GraphAuthority {
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
    pub fn from_profile_endpoint(endpoint: &Url) -> Result<Self, GraphError> {
        if endpoint.path() != "/v1.0/me" {
            return Err(GraphError::UnsafeLink);
        }
        let mut base_url = endpoint.clone();
        base_url.set_path(GRAPH_API_PATH_PREFIX);
        base_url.set_query(None);
        base_url.set_fragment(None);
        Self::from_base_url(base_url)
    }

    pub fn profile_url(&self, endpoint: &Url) -> Result<GraphUrl, GraphError> {
        self.opaque_link(endpoint.as_str())
    }

    pub fn root_folders_path(&self) -> Result<GraphUrl, GraphError> {
        let mut url = self
            .base_url
            .join("me/mailFolders")
            .map_err(|_| GraphError::UnsafeLink)?;
        url.query_pairs_mut().append_pair("$select", FOLDER_SELECT);
        self.validated_url(url)
    }

    pub fn well_known_folder_path(
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

    pub fn child_folders_path(&self, folder_id: &str) -> Result<GraphUrl, GraphError> {
        let mut url = self.folder_path(folder_id, "childFolders")?;
        url.query_pairs_mut().append_pair("$select", FOLDER_SELECT);
        self.validated_url(url)
    }

    pub fn folder_message_delta_path(&self, folder_id: &str) -> Result<GraphUrl, GraphError> {
        let mut url = self.folder_path(folder_id, "messages")?;
        url.path_segments_mut()
            .map_err(|()| GraphError::UnsafeLink)?
            .push("delta");
        url.query_pairs_mut().append_pair("$select", MESSAGE_SELECT);
        self.validated_url(url)
    }

    pub fn message_body_path(&self, message_id: &str) -> Result<GraphUrl, GraphError> {
        let mut url = self.message_path(message_id)?;
        url.query_pairs_mut().append_pair("$select", "body");
        self.validated_url(url)
    }

    pub fn message_update_path(&self, message_id: &str) -> Result<GraphUrl, GraphError> {
        self.validated_url(self.message_path(message_id)?)
    }

    pub fn message_move_path(&self, message_id: &str) -> Result<GraphUrl, GraphError> {
        let mut url = self.message_path(message_id)?;
        url.path_segments_mut()
            .map_err(|()| GraphError::UnsafeLink)?
            .push("move");
        self.validated_url(url)
    }

    pub fn message_reply_path(&self, message_id: &str) -> Result<GraphUrl, GraphError> {
        let mut url = self.message_path(message_id)?;
        url.path_segments_mut()
            .map_err(|()| GraphError::UnsafeLink)?
            .push("reply");
        self.validated_url(url)
    }

    pub fn mail_send_path(&self) -> Result<GraphUrl, GraphError> {
        let url = self
            .base_url
            .join("me/sendMail")
            .map_err(|_| GraphError::UnsafeLink)?;
        self.validated_url(url)
    }

    pub fn calendar_view_delta_path(
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

    pub fn primary_calendar_events_path(&self) -> Result<GraphUrl, GraphError> {
        let url = self
            .base_url
            .join("me/calendar/events")
            .map_err(|_| GraphError::UnsafeLink)?;
        self.validated_url(url)
    }

    pub fn calendar_event_path(&self, event_id: &str) -> Result<GraphUrl, GraphError> {
        self.validated_url(self.calendar_event_url(event_id)?)
    }

    pub fn editable_calendar_event_path(&self, event_id: &str) -> Result<GraphUrl, GraphError> {
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

    pub fn opaque_link(&self, link: &str) -> Result<GraphUrl, GraphError> {
        if has_user_info_separator(link) {
            return Err(GraphError::UnsafeLink);
        }
        let url = Url::parse(link).map_err(|_| GraphError::UnsafeLink)?;
        self.validate_url(&url)?;
        Ok(GraphUrl(link.to_owned()))
    }

    pub fn calendar_delta_opaque_link(&self, link: &str) -> Result<GraphUrl, GraphError> {
        if link.len() > MAX_GRAPH_CALENDAR_DELTA_URL_BYTES {
            return Err(GraphError::ResponseTooLarge);
        }
        self.opaque_link(link)
    }

    pub fn calendar_delta_reset_link(
        &self,
        range: &CalendarViewRange,
        location: &str,
    ) -> Result<GraphUrl, GraphError> {
        let reset_link = self.calendar_delta_opaque_link(location)?;
        let expected_url = Url::parse(self.calendar_view_delta_path(range)?.as_str())
            .map_err(|_| GraphError::UnsafeLink)?;
        let expected_start = range
            .start()
            .format(&Rfc3339)
            .map_err(|_| GraphError::InvalidRecord)?;
        let expected_end = range
            .end()
            .format(&Rfc3339)
            .map_err(|_| GraphError::InvalidRecord)?;
        let reset_url = Url::parse(reset_link.as_str()).map_err(|_| GraphError::UnsafeLink)?;
        let query = reset_url.query().ok_or(GraphError::UnsafeLink)?;
        if query
            .split('&')
            .any(|pair| !pair.contains('=') || !has_valid_percent_encoding(pair))
        {
            return Err(GraphError::UnsafeLink);
        }
        let mut has_start = false;
        let mut has_end = false;
        let mut has_empty_delta_token = false;
        for (name, value) in reset_url.query_pairs() {
            match name.as_ref() {
                "startDateTime" if !has_start && value == expected_start.as_str() => {
                    has_start = true;
                }
                "endDateTime" if !has_end && value == expected_end.as_str() => {
                    has_end = true;
                }
                "$deltatoken" if !has_empty_delta_token && value.is_empty() => {
                    has_empty_delta_token = true;
                }
                _ => return Err(GraphError::UnsafeLink),
            }
        }
        if reset_url.path() != expected_url.path()
            || !has_start
            || !has_end
            || !has_empty_delta_token
        {
            return Err(GraphError::UnsafeLink);
        }
        Ok(reset_link)
    }

    pub fn message_delta_reset_link(
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

fn has_valid_percent_encoding(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            index += 1;
        } else if bytes
            .get(index + 1)
            .is_some_and(|byte| byte.is_ascii_hexdigit())
            && bytes
                .get(index + 2)
                .is_some_and(|byte| byte.is_ascii_hexdigit())
        {
            index += 3;
        } else {
            return false;
        }
    }
    true
}

#[derive(Clone)]
pub struct GraphUrl(String);

impl GraphUrl {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[cfg(test)]
    pub fn fixture(url: Url) -> Self {
        Self(url.to_string())
    }
}

#[cfg(test)]
mod tests;
