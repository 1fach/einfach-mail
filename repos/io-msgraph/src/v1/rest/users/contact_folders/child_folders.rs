//! List the child folders of a Microsoft Graph contact folder
//! (`GET /me/contactFolders/{id}/childFolders`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/contactfolder-list-childfolders>

use alloc::format;

use io_http::rfc6750::bearer::HttpAuthBearer;
use log::{debug, trace};
use url::Url;

use crate::{
    coroutine::*,
    msgraph_try,
    v1::{
        query::to_query_pairs,
        rest::users::contact_folders::list::{
            MsgraphContactFoldersListParams, MsgraphContactFoldersListResponse,
        },
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

/// Lists the child folders of a Microsoft Graph contact folder.
pub struct MsgraphContactChildFoldersList {
    send: MsgraphSend<MsgraphContactFoldersListResponse>,
}

impl MsgraphContactChildFoldersList {
    /// Lists the child folders of the contact folder `id`, filtered by
    /// the OData `params`.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        id: &str,
        params: &MsgraphContactFoldersListParams,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph contact child folders listing");
        trace!("id: {id:?}");
        trace!("params: {params:?}");

        let user = user_path(user_id);
        let mut url = Url::parse(MSGRAPH_API_BASE)?
            .join(&format!("{user}/contactFolders/{id}/childFolders"))?;
        url.query_pairs_mut().extend_pairs(to_query_pairs(params));

        let send = MsgraphSend::get(auth, url);

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphContactChildFoldersList {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphContactFoldersListResponse>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("contact child folders listed");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
