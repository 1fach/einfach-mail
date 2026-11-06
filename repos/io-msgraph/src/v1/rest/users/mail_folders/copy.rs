//! Copy a Microsoft Graph mail folder to another folder
//! (`POST /me/mailFolders/{id}/copy`); returns the new copy.
//!
//! <https://learn.microsoft.com/en-us/graph/api/mailfolder-copy>

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

/// Body of the mail folder copy request (the destination folder id).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MsgraphMailFolderCopyRequest<'a> {
    destination_id: &'a str,
}

/// Copies a Microsoft Graph mail folder into another folder.
pub struct MsgraphMailFolderCopy {
    send: MsgraphSend<MsgraphMailFolder>,
}

impl MsgraphMailFolderCopy {
    /// Copies `id` into `destination` (a folder id or a well-known name
    /// such as `archive`).
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        id: &str,
        destination: &str,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph mail folder copy");
        trace!("id: {id:?}");
        trace!("destination: {destination:?}");

        let user = user_path(user_id);
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/mailFolders/{id}/copy"))?;
        let body = MsgraphMailFolderCopyRequest {
            destination_id: destination,
        };
        let send = MsgraphSend::post_json(auth, url, &body)?;

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphMailFolderCopy {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphMailFolder>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("mail folder copied");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
