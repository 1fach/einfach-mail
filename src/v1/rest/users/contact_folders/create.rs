//! Create a Microsoft Graph contact folder
//! (`POST /me/contactFolders`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/user-post-contactfolders>

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

/// Creates a Microsoft Graph contact folder.
pub struct MsgraphContactFolderCreate {
    send: MsgraphSend<MsgraphContactFolder>,
}

impl MsgraphContactFolderCreate {
    /// Creates `folder`, whose `display_name` must not be empty.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        folder: &MsgraphContactFolder,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph contact folder for creation");
        trace!("folder: {folder:?}");

        if folder.display_name.trim().is_empty() {
            let err =
                MsgraphSendError::InvalidRequest("Contact folder name cannot be empty".into());
            return Err(err);
        }

        let user = user_path(user_id);
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/contactFolders"))?;
        let send = MsgraphSend::post_json(auth, url, folder)?;

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphContactFolderCreate {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphContactFolder>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("contact folder created");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
