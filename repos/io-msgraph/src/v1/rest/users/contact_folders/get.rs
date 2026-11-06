//! Get a Microsoft Graph contact folder
//! (`GET /me/contactFolders/{id}`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/contactfolder-get>

use alloc::format;

use io_http::rfc6750::bearer::HttpAuthBearer;
use log::{debug, trace};
use url::Url;

use crate::{
    coroutine::*,
    msgraph_try,
    v1::{
        rest::users::contact_folders::MsgraphContactFolder,
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

/// Gets a Microsoft Graph contact folder.
pub struct MsgraphContactFolderGet {
    send: MsgraphSend<MsgraphContactFolder>,
}

impl MsgraphContactFolderGet {
    /// Gets the contact folder `id`.
    pub fn new(auth: &HttpAuthBearer, user_id: &str, id: &str) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph contact folder retrieval");
        trace!("id: {id:?}");

        let user = user_path(user_id);
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/contactFolders/{id}"))?;
        let send = MsgraphSend::get(auth, url);

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphContactFolderGet {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphContactFolder>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("contact folder retrieved");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
