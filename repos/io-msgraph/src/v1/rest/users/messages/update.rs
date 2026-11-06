//! Update a Microsoft Graph message (`PATCH /me/messages/{id}`); used to
//! change `isRead`, the follow-up flag, categories, etc.
//!
//! <https://learn.microsoft.com/en-us/graph/api/message-update>

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

/// Updates a Microsoft Graph message.
pub struct MsgraphMessageUpdate {
    send: MsgraphSend<MsgraphMessage>,
}

impl MsgraphMessageUpdate {
    /// Patches the message `id` with the set fields of `message`.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        id: &str,
        message: &MsgraphMessage,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph message update");
        trace!("id: {id:?}");
        trace!("message: {message:?}");

        let user = user_path(user_id);
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/messages/{id}"))?;
        let send = MsgraphSend::patch_json(auth, url, message)?;

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphMessageUpdate {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphMessage>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("message updated");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
