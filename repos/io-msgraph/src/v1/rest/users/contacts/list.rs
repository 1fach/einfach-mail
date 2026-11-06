//! List Microsoft Graph contacts (`GET /me/contacts` or
//! `GET /me/contactFolders/{id}/contacts`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/user-list-contacts>

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
        rest::users::contacts::MsgraphContact,
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

/// One page of contacts (`value` plus the OData paging link).
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct MsgraphContactsListResponse {
    /// The contacts of the page.
    #[serde(default)]
    pub value: Vec<MsgraphContact>,
    /// The URL of the next page, when one exists.
    #[serde(default, rename = "@odata.nextLink")]
    pub next_link: Option<String>,
}

/// OData query parameters for listing contacts.
#[derive(Debug, Clone, Default, Serialize, Eq, PartialEq)]
pub struct MsgraphContactsListParams<'a> {
    /// Maximum number of contacts per page (`$top`).
    #[serde(rename = "$top")]
    pub top: Option<u32>,
    /// Number of contacts to skip (`$skip`).
    #[serde(rename = "$skip")]
    pub skip: Option<u32>,
    /// Comma-separated properties to return (`$select`).
    #[serde(rename = "$select")]
    pub select: Option<&'a str>,
    /// OData filter expression (`$filter`).
    #[serde(rename = "$filter")]
    pub filter: Option<&'a str>,
    /// Comma-separated sort properties (`$orderby`).
    #[serde(rename = "$orderby")]
    pub orderby: Option<&'a str>,
    /// Navigation clause to expand (`$expand`).
    #[serde(rename = "$expand")]
    pub expand: Option<&'a str>,
    /// Whether the total count rides along the page (`$count`).
    #[serde(rename = "$count")]
    pub count: Option<bool>,
}

/// Lists the Microsoft Graph contacts of a contact folder.
pub struct MsgraphContactsList {
    send: MsgraphSend<MsgraphContactsListResponse>,
}

impl MsgraphContactsList {
    /// Lists contacts in the default Contacts folder, or in `folder`
    /// when given (a contact folder id).
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        folder: Option<&str>,
        params: &MsgraphContactsListParams,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph contacts listing");
        trace!("folder: {folder:?}");
        trace!("params: {params:?}");

        let user = user_path(user_id);
        let path = match folder {
            Some(folder) => format!("{user}/contactFolders/{folder}/contacts"),
            None => format!("{user}/contacts"),
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

impl MsgraphCoroutine for MsgraphContactsList {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphContactsListResponse>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("contacts listed");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
