//! List the Microsoft Graph mail folders (`GET /me/mailFolders`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/user-list-mailfolders>

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
        rest::users::mail_folders::MsgraphMailFolder,
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

/// One page of mail folders (`value` plus the OData paging link).
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct MsgraphMailFoldersListResponse {
    /// The mail folders of the page.
    #[serde(default)]
    pub value: Vec<MsgraphMailFolder>,
    /// The URL of the next page, when one exists.
    #[serde(default, rename = "@odata.nextLink")]
    pub next_link: Option<String>,
}

/// OData query parameters for listing mail folders.
#[derive(Debug, Clone, Default, Serialize, Eq, PartialEq)]
pub struct MsgraphMailFoldersListParams<'a> {
    /// Maximum number of folders per page (`$top`).
    #[serde(rename = "$top")]
    pub top: Option<u32>,
    /// Number of folders to skip (`$skip`).
    #[serde(rename = "$skip")]
    pub skip: Option<u32>,
    /// Comma-separated properties to return (`$select`).
    #[serde(rename = "$select")]
    pub select: Option<&'a str>,
    /// Whether hidden folders are included in the listing.
    #[serde(rename = "includeHiddenFolders")]
    pub include_hidden_folders: Option<bool>,
}

/// Lists the Microsoft Graph mail folders of a mailbox.
pub struct MsgraphMailFoldersList {
    send: MsgraphSend<MsgraphMailFoldersListResponse>,
}

impl MsgraphMailFoldersList {
    /// Lists the top-level mail folders, filtered by the OData
    /// `params`.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        params: &MsgraphMailFoldersListParams,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph mail folders listing");
        trace!("params: {params:?}");

        let user = user_path(user_id);
        let mut url = Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/mailFolders"))?;
        url.query_pairs_mut().extend_pairs(to_query_pairs(params));

        let send = MsgraphSend::get(auth, url);

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphMailFoldersList {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphMailFoldersListResponse>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("mail folders listed");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
