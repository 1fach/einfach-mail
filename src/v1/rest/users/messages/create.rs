//! Create a Microsoft Graph draft message (`POST /me/messages` or
//! `POST /me/mailFolders/{id}/messages`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/user-post-messages>

use alloc::format;

use io_http::rfc6750::bearer::HttpAuthBearer;
use log::{debug, trace};
use url::Url;

use crate::{
    coroutine::*,
    msgraph_try,
    v1::{
        rest::users::messages::MsgraphMessage,
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

/// Creates a Microsoft Graph draft message from JSON.
pub struct MsgraphMessageCreate {
    send: MsgraphSend<MsgraphMessage>,
}

impl MsgraphMessageCreate {
    /// Creates the draft in the Drafts folder, or in `folder` when given
    /// (a folder id or a well-known name such as `drafts`).
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        folder: Option<&str>,
        message: &MsgraphMessage,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph message for creation");
        trace!("folder: {folder:?}");
        trace!("message: {message:?}");

        let user = user_path(user_id);
        let path = match folder {
            Some(folder) => format!("{user}/mailFolders/{folder}/messages"),
            None => format!("{user}/messages"),
        };
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&path)?;
        let send = MsgraphSend::post_json(auth, url, message)?;

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphMessageCreate {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphMessage>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("message created");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
