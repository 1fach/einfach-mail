//! List the Microsoft Graph contact folders
//! (`GET /me/contactFolders`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/user-list-contactfolders>

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
        rest::users::contact_folders::MsgraphContactFolder,
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

/// One page of contact folders (`value` plus the OData paging link).
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct MsgraphContactFoldersListResponse {
    /// The contact folders of the page.
    #[serde(default)]
    pub value: Vec<MsgraphContactFolder>,
    /// The URL of the next page, when one exists.
    #[serde(default, rename = "@odata.nextLink")]
    pub next_link: Option<String>,
}

/// OData query parameters for listing contact folders.
#[derive(Debug, Clone, Default, Serialize, Eq, PartialEq)]
pub struct MsgraphContactFoldersListParams<'a> {
    /// Maximum number of folders per page (`$top`).
    #[serde(rename = "$top")]
    pub top: Option<u32>,
    /// Number of folders to skip (`$skip`).
    #[serde(rename = "$skip")]
    pub skip: Option<u32>,
    /// Comma-separated properties to return (`$select`).
    #[serde(rename = "$select")]
    pub select: Option<&'a str>,
}

/// Lists the Microsoft Graph contact folders of a mailbox.
pub struct MsgraphContactFoldersList {
    send: MsgraphSend<MsgraphContactFoldersListResponse>,
}

impl MsgraphContactFoldersList {
    /// Lists the top-level contact folders, filtered by the OData
    /// `params`.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        params: &MsgraphContactFoldersListParams,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph contact folders listing");
        trace!("params: {params:?}");

        let user = user_path(user_id);
        let mut url = Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/contactFolders"))?;
        url.query_pairs_mut().extend_pairs(to_query_pairs(params));

        let send = MsgraphSend::get(auth, url);

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphContactFoldersList {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphContactFoldersListResponse>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("contact folders listed");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
