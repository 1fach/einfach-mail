//! List Microsoft Graph messages (`GET /me/messages` or
//! `GET /me/mailFolders/{id}/messages`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/user-list-messages>

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
        rest::users::messages::MsgraphMessage,
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

/// One page of messages (`value` plus the OData paging link).
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct MsgraphMessagesListResponse {
    /// The messages of the page.
    #[serde(default)]
    pub value: Vec<MsgraphMessage>,
    /// The URL of the next page, when one exists.
    #[serde(default, rename = "@odata.nextLink")]
    pub next_link: Option<String>,
}

/// OData query parameters for listing messages.
#[derive(Debug, Clone, Default, Serialize, Eq, PartialEq)]
pub struct MsgraphMessagesListParams<'a> {
    /// Maximum number of messages per page (`$top`).
    #[serde(rename = "$top")]
    pub top: Option<u32>,
    /// Number of messages to skip (`$skip`).
    #[serde(rename = "$skip")]
    pub skip: Option<u32>,
    /// Comma-separated properties to return (`$select`).
    #[serde(rename = "$select")]
    pub select: Option<&'a str>,
    /// OData filter expression (`$filter`).
    #[serde(rename = "$filter")]
    pub filter: Option<&'a str>,
    /// Navigation properties to expand (`$expand`), such as
    /// `singleValueExtendedProperties($filter=id eq 'Integer 0x0E08')`.
    #[serde(rename = "$expand")]
    pub expand: Option<&'a str>,
    /// Comma-separated sort properties (`$orderby`).
    #[serde(rename = "$orderby")]
    pub orderby: Option<&'a str>,
    /// Whether the total count rides along the page (`$count`).
    #[serde(rename = "$count")]
    pub count: Option<bool>,
    /// OData `$search` over the message collection (e.g. `"subject:foo"`
    /// or a bare term); Graph wraps a bare term as a free-text search.
    #[serde(rename = "$search")]
    pub search: Option<&'a str>,
}

/// Lists the Microsoft Graph messages of a mailbox or mail folder.
pub struct MsgraphMessagesList {
    send: MsgraphSend<MsgraphMessagesListResponse>,
}

impl MsgraphMessagesList {
    /// Lists messages in the whole mailbox, or in `folder` when given (a
    /// folder id or a well-known name such as `inbox`).
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        folder: Option<&str>,
        params: &MsgraphMessagesListParams,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph messages listing");
        trace!("folder: {folder:?}");
        trace!("params: {params:?}");

        let user = user_path(user_id);
        let path = match folder {
            Some(folder) => format!("{user}/mailFolders/{folder}/messages"),
            None => format!("{user}/messages"),
        };
        let mut url = Url::parse(MSGRAPH_API_BASE)?.join(&path)?;
        url.query_pairs_mut().extend_pairs(to_query_pairs(params));

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

impl MsgraphCoroutine for MsgraphMessagesList {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphMessagesListResponse>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("messages listed");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
