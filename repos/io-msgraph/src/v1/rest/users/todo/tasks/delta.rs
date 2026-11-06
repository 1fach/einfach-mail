//! Track changes to the tasks of a Microsoft To Do list
//! (`GET /me/todo/lists/{id}/tasks/delta`).
//!
//! An initial request enumerates every task of the list and ends with an
//! `@odata.deltaLink`; following that link returns only what changed
//! since, removals arriving as `@removed`-marked rows. An expired link
//! answers HTTP 410; the consumer falls back to an initial request.
//!
//! A delta refuses to `$expand` extended properties: a consumer needing
//! them re-reads each changed task by id.
//!
//! <https://learn.microsoft.com/en-us/graph/api/todotask-delta>

use alloc::{format, string::String, vec::Vec};

use io_http::rfc6750::bearer::HttpAuthBearer;
use log::{debug, trace};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    coroutine::*,
    msgraph_try,
    v1::{
        rest::users::{contacts::delta::MsgraphRemoved, todo::tasks::MsgraphTodoTask},
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

/// One page of a tasks delta round.
///
/// More pages follow through `next_link`; the round ends when
/// `delta_link` arrives (the token of the next round).
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct MsgraphTodoTasksDeltaResponse {
    /// The changed tasks of the page.
    #[serde(default)]
    pub value: Vec<MsgraphTodoTaskDelta>,
    /// The URL of the next page of the round, when one exists.
    #[serde(default, rename = "@odata.nextLink")]
    pub next_link: Option<String>,
    /// The URL closing the round, carrying the next round's token.
    #[serde(default, rename = "@odata.deltaLink")]
    pub delta_link: Option<String>,
}

/// One task row of a delta page: the task (only its id when the row is
/// a removal), plus the `@removed` marker.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct MsgraphTodoTaskDelta {
    /// The changed task.
    #[serde(flatten)]
    pub task: MsgraphTodoTask,
    /// The removal marker, present when the row is a removal.
    #[serde(default, rename = "@removed", skip_serializing_if = "Option::is_none")]
    pub removed: Option<MsgraphRemoved>,
}

/// I/O-free coroutine for a tasks delta request, opening a round with
/// [`new`](Self::new) or continuing one with
/// [`from_link`](Self::from_link).
pub struct MsgraphTodoTasksDelta {
    send: MsgraphSend<MsgraphTodoTasksDeltaResponse>,
}

impl MsgraphTodoTasksDelta {
    /// Starts a delta round over the tasks of the list `list_id`.
    ///
    /// `select` trims each row to the named properties (the id always
    /// rides along).
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        list_id: &str,
        select: Option<&str>,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph to do tasks delta");
        trace!("list: {list_id:?}, select: {select:?}");

        let user = user_path(user_id);
        let path = format!("{user}/todo/lists/{list_id}/tasks/delta");
        let mut url = Url::parse(MSGRAPH_API_BASE)?.join(&path)?;
        if let Some(select) = select {
            url.query_pairs_mut().append_pair("$select", select);
        }

        let send = MsgraphSend::get(auth, url);

        Ok(Self { send })
    }

    /// Continues a round from an `@odata.nextLink`, or starts the next
    /// round from a saved `@odata.deltaLink`, sent as-is.
    pub fn from_link(auth: &HttpAuthBearer, link: &str) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph to do tasks delta from link");
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

impl MsgraphCoroutine for MsgraphTodoTasksDelta {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphTodoTasksDeltaResponse>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("to do tasks delta page received");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
