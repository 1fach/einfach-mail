//! Track changes to Microsoft Graph messages
//! (`GET /me/messages/delta` or
//! `GET /me/mailFolders/{id}/messages/delta`).
//!
//! An initial request (no delta link) enumerates every message and
//! ends with an `@odata.deltaLink`; feeding that link back through
//! [`MsgraphMessagesDelta::from_link`] returns only what changed
//! since. Changed rows carry the full message, or only the properties
//! named by `$select`. The delta endpoint takes no `$search`, and a
//! `$filter` only on the reception date (`receivedDateTime ge
//! {instant}` or `gt`); see [`MsgraphMessagesDeltaParams`]. Pages
//! hold 10 messages unless the request prefers more
//! ([`MsgraphMessagesDelta::max_page_size`]). Removals arrive as
//! `@removed`-marked rows carrying a reason. A round is over when
//! `@odata.deltaLink` replaces `@odata.nextLink` in the page. An
//! expired link answers HTTP 410; the consumer falls back to an
//! initial request.
//!
//! <https://learn.microsoft.com/en-us/graph/api/message-delta>

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
        rest::users::{contacts::delta::MsgraphRemoved, messages::MsgraphMessage},
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

/// One page of a messages delta round.
///
/// More pages follow through `next_link`; the round ends when
/// `delta_link` arrives (the token of the next round).
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct MsgraphMessagesDeltaResponse {
    /// The changed messages of the page.
    #[serde(default)]
    pub value: Vec<MsgraphMessageDelta>,
    /// The URL of the next page of the round, when one exists.
    #[serde(default, rename = "@odata.nextLink")]
    pub next_link: Option<String>,
    /// The URL closing the round, carrying the next round's token.
    #[serde(default, rename = "@odata.deltaLink")]
    pub delta_link: Option<String>,
}

/// One message row of a delta page: the message (only its id when the
/// row is a removal), plus the `@removed` marker.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct MsgraphMessageDelta {
    /// The changed message.
    #[serde(flatten)]
    pub message: MsgraphMessage,
    /// The removal marker, present when the row is a removal.
    #[serde(default, rename = "@removed", skip_serializing_if = "Option::is_none")]
    pub removed: Option<MsgraphRemoved>,
}

/// OData options of a messages delta round, sent on its first request
/// (the links Graph hands back carry them on).
#[derive(Debug, Clone, Default, Serialize, Eq, PartialEq)]
pub struct MsgraphMessagesDeltaParams<'a> {
    /// Comma-separated properties to return (`$select`); the id always
    /// rides along.
    #[serde(rename = "$select")]
    pub select: Option<&'a str>,
    /// OData filter expression (`$filter`). The delta endpoint only
    /// takes one on the reception date, such as
    /// `receivedDateTime ge 2026-01-01T00:00:00Z`.
    #[serde(rename = "$filter")]
    pub filter: Option<&'a str>,
    /// Navigation properties to expand (`$expand`), such as
    /// `singleValueExtendedProperties($filter=id eq 'Integer 0x0E08')`.
    #[serde(rename = "$expand")]
    pub expand: Option<&'a str>,
    /// At most this many messages per page
    /// (`Prefer: odata.maxpagesize`), a header rather than a query
    /// option.
    #[serde(skip)]
    pub max_page_size: Option<u32>,
}

/// I/O-free coroutine for one messages delta request: the initial
/// request through [`new`](Self::new), later pages and rounds through
/// [`from_link`](Self::from_link).
pub struct MsgraphMessagesDelta {
    send: MsgraphSend<MsgraphMessagesDeltaResponse>,
}

impl MsgraphMessagesDelta {
    /// Starts a delta round over the whole mailbox, or over `folder`
    /// when given (a folder id or a well-known name such as `inbox`).
    ///
    /// `select` trims each row to the named properties (the id always
    /// rides along).
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        folder: Option<&str>,
        select: Option<&str>,
    ) -> Result<Self, MsgraphSendError> {
        let params = MsgraphMessagesDeltaParams {
            select,
            ..Default::default()
        };

        Self::with_params(auth, user_id, folder, &params)
    }

    /// Starts a delta round like [`new`](Self::new), with the OData
    /// options of `params`. Its page size, when set, goes on this first
    /// request only: chain [`max_page_size`](Self::max_page_size) on
    /// every [`from_link`](Self::from_link) of the round.
    pub fn with_params(
        auth: &HttpAuthBearer,
        user_id: &str,
        folder: Option<&str>,
        params: &MsgraphMessagesDeltaParams,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph messages delta");
        trace!("folder: {folder:?}");
        trace!("params: {params:?}");

        let user = user_path(user_id);
        let path = match folder {
            Some(folder) => format!("{user}/mailFolders/{folder}/messages/delta"),
            None => format!("{user}/messages/delta"),
        };
        let mut url = Url::parse(MSGRAPH_API_BASE)?.join(&path)?;
        url.query_pairs_mut().extend_pairs(to_query_pairs(params));

        let mut send = MsgraphSend::get(auth, url);
        if let Some(size) = params.max_page_size {
            send = send.max_page_size(size);
        }

        Ok(Self { send })
    }

    /// Continues a round from an `@odata.nextLink`, or starts the next
    /// round from a saved `@odata.deltaLink`.
    ///
    /// The link already carries the server-issued token, so it is sent
    /// as-is through a plain GET.
    pub fn from_link(auth: &HttpAuthBearer, link: &str) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph messages delta from link");
        trace!("link: {link:?}");

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

impl MsgraphCoroutine for MsgraphMessagesDelta {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphMessagesDeltaResponse>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("messages delta page received");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
