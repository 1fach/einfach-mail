//! List Microsoft Graph calendars (`GET /me/calendars`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/user-list-calendars>

use alloc::{format, string::String, vec::Vec};

use io_http::rfc6750::bearer::HttpAuthBearer;
use log::{debug, trace};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    coroutine::*,
    msgraph_try,
    v1::{
        query::to_query_pairs,
        rest::users::calendars::MsgraphCalendar,
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

/// One page of calendars (`value` plus the OData paging link).
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct MsgraphCalendarsListResponse {
    /// The calendars of the page.
    #[serde(default)]
    pub value: Vec<MsgraphCalendar>,
    /// The URL of the next page, when one exists.
    #[serde(default, rename = "@odata.nextLink")]
    pub next_link: Option<String>,
}

/// OData query parameters for listing calendars.
#[derive(Debug, Clone, Default, Serialize, Eq, PartialEq)]
pub struct MsgraphCalendarsListParams<'a> {
    /// Maximum number of calendars per page (`$top`).
    #[serde(rename = "$top")]
    pub top: Option<u32>,
    /// Number of calendars to skip (`$skip`).
    #[serde(rename = "$skip")]
    pub skip: Option<u32>,
    /// Comma-separated properties to return (`$select`).
    #[serde(rename = "$select")]
    pub select: Option<&'a str>,
    /// OData filter expression (`$filter`).
    #[serde(rename = "$filter")]
    pub filter: Option<&'a str>,
}

/// Lists the Microsoft Graph calendars of the user.
pub struct MsgraphCalendarsList {
    send: MsgraphSend<MsgraphCalendarsListResponse>,
}

impl MsgraphCalendarsList {
    /// Lists the user's calendars.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        params: &MsgraphCalendarsListParams,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph calendars listing");
        trace!("params: {params:?}");

        let user = user_path(user_id);
        let mut url = Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/calendars"))?;
        url.query_pairs_mut().extend_pairs(to_query_pairs(params));

        let send = MsgraphSend::get(auth, url);

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphCalendarsList {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphCalendarsListResponse>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("calendars listed");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
