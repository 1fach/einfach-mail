//! Track changes to Microsoft Graph contacts (`GET /me/contacts/delta`
//! or `GET /me/contactFolders/{id}/contacts/delta`).
//!
//! An initial request (no delta link) enumerates every contact and
//! ends with an `@odata.deltaLink`; feeding that link back through
//! [`MsgraphSend`] returns only what changed since, removals arriving
//! as `@removed`-marked rows. An expired link answers HTTP 410; the
//! consumer falls back to an initial request.
//!
//! <https://learn.microsoft.com/en-us/graph/api/contact-delta>

use alloc::{format, string::String, vec::Vec};

use io_http::rfc6750::bearer::HttpAuthBearer;
use log::{debug, trace};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    coroutine::*,
    msgraph_try,
    v1::{
        rest::users::contacts::MsgraphContact,
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

/// One page of a contacts delta round.
///
/// More pages follow through `next_link`; the round ends when
/// `delta_link` arrives (the token of the next round).
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct MsgraphContactsDeltaResponse {
    /// The changed contacts of the page.
    #[serde(default)]
    pub value: Vec<MsgraphContactDelta>,
    /// The URL of the next page of the round, when one exists.
    #[serde(default, rename = "@odata.nextLink")]
    pub next_link: Option<String>,
    /// The URL closing the round, carrying the next round's token.
    #[serde(default, rename = "@odata.deltaLink")]
    pub delta_link: Option<String>,
}

/// One contact row of a delta page: the contact (only its id when the
/// row is a removal), plus the `@removed` marker.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct MsgraphContactDelta {
    /// The changed contact.
    #[serde(flatten)]
    pub contact: MsgraphContact,
    /// The removal marker, present when the row is a removal.
    #[serde(default, rename = "@removed", skip_serializing_if = "Option::is_none")]
    pub removed: Option<MsgraphRemoved>,
}

/// The `@removed` marker of a delta row.
///
/// <https://learn.microsoft.com/en-us/graph/delta-query-overview>
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct MsgraphRemoved {
    /// `deleted` for a hard delete, `changed` for an item that left
    /// the queried scope.
    #[serde(default)]
    pub reason: String,
}

/// I/O-free coroutine for a contacts delta request, opening a round
/// with [`new`](Self::new) or continuing one with
/// [`from_link`](Self::from_link).
pub struct MsgraphContactsDelta {
    send: MsgraphSend<MsgraphContactsDeltaResponse>,
}

impl MsgraphContactsDelta {
    /// Starts a delta round over the default Contacts folder, or over
    /// `folder` when given (a contact folder id).
    ///
    /// `select` trims each row to the named properties (the id always
    /// rides along).
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        folder: Option<&str>,
        select: Option<&str>,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph contacts delta");
        trace!("folder: {folder:?}");

        let user = user_path(user_id);
        let path = match folder {
            Some(folder) => format!("{user}/contactFolders/{folder}/contacts/delta"),
            None => format!("{user}/contacts/delta"),
        };
        let mut url = Url::parse(MSGRAPH_API_BASE)?.join(&path)?;
        if let Some(select) = select {
            url.query_pairs_mut().append_pair("$select", select);
        }

        let send = MsgraphSend::get(auth, url);

        Ok(Self { send })
    }

    /// Continues a round from an `@odata.nextLink`, or starts the next
    /// round from a saved `@odata.deltaLink`.
    ///
    /// The link already carries the folder, the `$select` and the
    /// server-issued token, so it is sent as-is through a plain GET.
    pub fn from_link(auth: &HttpAuthBearer, link: &str) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph contacts delta from link");
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

impl MsgraphCoroutine for MsgraphContactsDelta {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphContactsDeltaResponse>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("contacts delta page received");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
