//! Create a Microsoft Graph mail folder (`POST /me/mailFolders`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/user-post-mailfolders>

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

/// Creates a Microsoft Graph mail folder.
pub struct MsgraphMailFolderCreate {
    send: MsgraphSend<MsgraphMailFolder>,
}

impl MsgraphMailFolderCreate {
    /// Creates `folder`, whose `display_name` must not be empty.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        folder: &MsgraphMailFolder,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph mail folder for creation");
        trace!("folder: {folder:?}");

        if folder.display_name.trim().is_empty() {
            let err = MsgraphSendError::InvalidRequest("Mail folder name cannot be empty".into());
            return Err(err);
        }

        let user = user_path(user_id);
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/mailFolders"))?;
        let send = MsgraphSend::post_json(auth, url, folder)?;

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphMailFolderCreate {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphMailFolder>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("mail folder created");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
