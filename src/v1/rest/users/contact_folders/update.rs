//! Update a Microsoft Graph contact folder
//! (`PATCH /me/contactFolders/{id}`); used to rename a folder via its
//! `displayName`.
//!
//! <https://learn.microsoft.com/en-us/graph/api/contactfolder-update>

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

/// Updates a Microsoft Graph contact folder.
pub struct MsgraphContactFolderUpdate {
    send: MsgraphSend<MsgraphContactFolder>,
}

impl MsgraphContactFolderUpdate {
    /// Patches the contact folder `id` with the set fields of
    /// `folder`.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        id: &str,
        folder: &MsgraphContactFolder,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph contact folder update");
        trace!("id: {id:?}");
        trace!("folder: {folder:?}");

        let user = user_path(user_id);
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/contactFolders/{id}"))?;
        let send = MsgraphSend::patch_json(auth, url, folder)?;

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphContactFolderUpdate {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphContactFolder>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("contact folder updated");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
