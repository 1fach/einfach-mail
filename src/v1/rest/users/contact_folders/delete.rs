//! Delete a Microsoft Graph contact folder
//! (`DELETE /me/contactFolders/{id}`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/contactfolder-delete>

use alloc::format;

use io_http::rfc6750::bearer::HttpAuthBearer;
use log::{debug, trace};
use url::Url;

use crate::{
    coroutine::*,
    msgraph_try,
    v1::send::{
        MSGRAPH_API_BASE, MsgraphNoResponse, MsgraphSend, MsgraphSendError, MsgraphSendOutput,
        user_path,
    },
};

/// Deletes a Microsoft Graph contact folder.
pub struct MsgraphContactFolderDelete {
    send: MsgraphSend<MsgraphNoResponse>,
}

impl MsgraphContactFolderDelete {
    /// Deletes the contact folder `id`.
    pub fn new(auth: &HttpAuthBearer, user_id: &str, id: &str) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph contact folder deletion");
        trace!("id: {id:?}");

        let user = user_path(user_id);
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/contactFolders/{id}"))?;
        let send = MsgraphSend::delete(auth, url);

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphContactFolderDelete {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphNoResponse>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("contact folder deleted");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
