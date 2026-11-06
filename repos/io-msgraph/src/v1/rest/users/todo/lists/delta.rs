//! Track changes to Microsoft To Do task lists
//! (`GET /me/todo/lists/delta`).
//!
//! An initial request enumerates every list and ends with an
//! `@odata.deltaLink`; following that link returns only what changed
//! since, removals arriving as `@removed`-marked rows. An expired link
//! answers HTTP 410; the consumer falls back to an initial request.
//!
//! <https://learn.microsoft.com/en-us/graph/api/todotasklist-delta>

use alloc::{format, string::String, vec::Vec};

use io_http::rfc6750::bearer::HttpAuthBearer;
use log::{debug, trace};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    coroutine::*,
    msgraph_try,
    v1::{
        rest::users::{contacts::delta::MsgraphRemoved, todo::lists::MsgraphTodoList},
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

/// One page of a task lists delta round.
///
/// More pages follow through `next_link`; the round ends when
/// `delta_link` arrives (the token of the next round).
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct MsgraphTodoListsDeltaResponse {
    /// The changed lists of the page.
    #[serde(default)]
    pub value: Vec<MsgraphTodoListDelta>,
    /// The URL of the next page of the round, when one exists.
    #[serde(default, rename = "@odata.nextLink")]
    pub next_link: Option<String>,
    /// The URL closing the round, carrying the next round's token.
    #[serde(default, rename = "@odata.deltaLink")]
    pub delta_link: Option<String>,
}

/// One list row of a delta page: the list (only its id when the row is
/// a removal), plus the `@removed` marker.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct MsgraphTodoListDelta {
    /// The changed list.
    #[serde(flatten)]
    pub list: MsgraphTodoList,
    /// The removal marker, present when the row is a removal.
    #[serde(default, rename = "@removed", skip_serializing_if = "Option::is_none")]
    pub removed: Option<MsgraphRemoved>,
}

/// I/O-free coroutine for a task lists delta request, opening a round
/// with [`new`](Self::new) or continuing one with
/// [`from_link`](Self::from_link).
pub struct MsgraphTodoListsDelta {
    send: MsgraphSend<MsgraphTodoListsDeltaResponse>,
}

impl MsgraphTodoListsDelta {
    /// Starts a delta round over the user's task lists.
    pub fn new(auth: &HttpAuthBearer, user_id: &str) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph to do lists delta");

        let user = user_path(user_id);
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/todo/lists/delta"))?;
        let send = MsgraphSend::get(auth, url);

        Ok(Self { send })
    }

    /// Continues a round from an `@odata.nextLink`, or starts the next
    /// round from a saved `@odata.deltaLink`, sent as-is.
    pub fn from_link(auth: &HttpAuthBearer, link: &str) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph to do lists delta from link");
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

impl MsgraphCoroutine for MsgraphTodoListsDelta {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphTodoListsDeltaResponse>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("to do lists delta page received");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
