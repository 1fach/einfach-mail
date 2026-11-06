//! Move a Microsoft Graph mail folder to another folder
//! (`POST /me/mailFolders/{id}/move`); returns the moved folder.
//!
//! <https://learn.microsoft.com/en-us/graph/api/mailfolder-move>

use alloc::format;

use io_http::rfc6750::bearer::HttpAuthBearer;
use log::{debug, trace};
use serde::Serialize;
use url::Url;

use crate::{
    coroutine::*,
    msgraph_try,
    v1::{
        rest::users::mail_folders::MsgraphMailFolder,
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

/// Body of the mail folder move request (the destination folder id).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MsgraphMailFolderMoveRequest<'a> {
    destination_id: &'a str,
}

/// Moves a Microsoft Graph mail folder into another folder.
pub struct MsgraphMailFolderMove {
    send: MsgraphSend<MsgraphMailFolder>,
}

impl MsgraphMailFolderMove {
    /// Moves `id` into `destination` (a folder id or a well-known name
    /// such as `archive`).
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        id: &str,
        destination: &str,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph mail folder move");
        trace!("id: {id:?}");
        trace!("destination: {destination:?}");

        let user = user_path(user_id);
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/mailFolders/{id}/move"))?;
        let body = MsgraphMailFolderMoveRequest {
            destination_id: destination,
        };
        let send = MsgraphSend::post_json(auth, url, &body)?;

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphMailFolderMove {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphMailFolder>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("mail folder moved");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
