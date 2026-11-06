//! Get a Microsoft Graph mail folder (`GET /me/mailFolders/{id}`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/mailfolder-get>

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

/// Gets a Microsoft Graph mail folder.
pub struct MsgraphMailFolderGet {
    send: MsgraphSend<MsgraphMailFolder>,
}

impl MsgraphMailFolderGet {
    /// Gets the mail folder `id` (an id or a well-known name such as
    /// `inbox`).
    pub fn new(auth: &HttpAuthBearer, user_id: &str, id: &str) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph mail folder retrieval");
        trace!("id: {id:?}");

        let user = user_path(user_id);
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/mailFolders/{id}"))?;
        let send = MsgraphSend::get(auth, url);

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphMailFolderGet {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphMailFolder>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("mail folder retrieved");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
