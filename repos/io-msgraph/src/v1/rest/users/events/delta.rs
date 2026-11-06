//! Microsoft Graph calendar view delta (`GET /me/calendarView/delta` or
//! `GET /me/calendars/{id}/calendarView/delta`).
//!
//! The changes to the lone events, occurrences and exceptions of a window
//! since the previous round. The window is fixed by the round's first
//! request and carried by every link after it.
//!
//! <https://learn.microsoft.com/en-us/graph/api/event-delta>

use alloc::{format, string::String, vec::Vec};

use io_http::rfc6750::bearer::HttpAuthBearer;
use log::{debug, trace};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    coroutine::*,
    msgraph_try,
    v1::{
        rest::users::{contacts::delta::MsgraphRemoved, events::MsgraphEvent},
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

/// One page of a calendar view delta round.
///
/// More pages follow through `next_link`; the round ends when
/// `delta_link` arrives (the token of the next round).
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct MsgraphEventsDeltaResponse {
    /// The changed events of the page.
    #[serde(default)]
    pub value: Vec<MsgraphEventDelta>,
    /// The URL of the next page of the round, when one exists.
    #[serde(default, rename = "@odata.nextLink")]
    pub next_link: Option<String>,
    /// The URL closing the round, carrying the next round's token.
    #[serde(default, rename = "@odata.deltaLink")]
    pub delta_link: Option<String>,
}

/// One event row of a delta page: the event (only its id when the row is
/// a removal), plus the `@removed` marker.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct MsgraphEventDelta {
    /// The changed event.
    #[serde(flatten)]
    pub event: MsgraphEvent,
    /// The removal marker, present when the row is a removal.
    #[serde(default, rename = "@removed", skip_serializing_if = "Option::is_none")]
    pub removed: Option<MsgraphRemoved>,
}

/// I/O-free coroutine for a calendar view delta request, opening a round
/// with [`new`](Self::new) or continuing one with
/// [`from_link`](Self::from_link).
pub struct MsgraphEventsDelta {
    send: MsgraphSend<MsgraphEventsDeltaResponse>,
}

impl MsgraphEventsDelta {
    /// Starts a delta round over the view of the default calendar, or of
    /// `calendar` when given, between `start` and `end`, ISO 8601
    /// date-times.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        calendar: Option<&str>,
        start: &str,
        end: &str,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph calendar view delta");
        trace!("calendar: {calendar:?}, start: {start}, end: {end}");

        let user = user_path(user_id);
        let path = match calendar {
            Some(calendar) => format!("{user}/calendars/{calendar}/calendarView/delta"),
            None => format!("{user}/calendarView/delta"),
        };
        let mut url = Url::parse(MSGRAPH_API_BASE)?.join(&path)?;
        url.query_pairs_mut()
            .append_pair("startDateTime", start)
            .append_pair("endDateTime", end);

        let send = MsgraphSend::get(auth, url);

        Ok(Self { send })
    }

    /// Continues a round from an `@odata.nextLink`, or starts the next
    /// round from a saved `@odata.deltaLink`, sent as-is.
    pub fn from_link(auth: &HttpAuthBearer, link: &str) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph calendar view delta from link");
        trace!("link: {link}");

        let url = Url::parse(link)?;
        let send = MsgraphSend::get(auth, url);

        Ok(Self { send })
    }

    /// Asks Graph for at most `size` items per page
    /// (`Prefer: odata.maxpagesize={size}`), see
    /// [`MsgraphSend::max_page_size`]. Graph keeps no memory of it
    /// across pages: chain it on every page request, the ones built
    /// from an `@odata.nextLink` included.
    pub fn max_page_size(mut self, size: u32) -> Self {
        self.send = self.send.max_page_size(size);
        self
    }
}

impl MsgraphCoroutine for MsgraphEventsDelta {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphEventsDeltaResponse>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("calendar view delta received");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
