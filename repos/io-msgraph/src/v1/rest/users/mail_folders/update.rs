//! Update a Microsoft Graph mail folder (`PATCH /me/mailFolders/{id}`);
//! used to rename a folder via its `displayName`.
//!
//! <https://learn.microsoft.com/en-us/graph/api/mailfolder-update>

use alloc::format;

use io_http::rfc6750::bearer::HttpAuthBearer;
use log::{debug, trace};
use url::Url;

use crate::{
    coroutine::*,
    msgraph_try,
    v1::{
        rest::users::mail_folders::MsgraphMailFolder,
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

/// Updates a Microsoft Graph mail folder.
pub struct MsgraphMailFolderUpdate {
    send: MsgraphSend<MsgraphMailFolder>,
}

impl MsgraphMailFolderUpdate {
    /// Patches the mail folder `id` with the set fields of `folder`.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        id: &str,
        folder: &MsgraphMailFolder,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph mail folder update");
        trace!("id: {id:?}");
        trace!("folder: {folder:?}");

        let user = user_path(user_id);
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/mailFolders/{id}"))?;
        let send = MsgraphSend::patch_json(auth, url, folder)?;

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphMailFolderUpdate {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphMailFolder>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("mail folder updated");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
