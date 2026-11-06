//! Send a Microsoft Graph draft message
//! (`POST /me/messages/{id}/send`); the draft is moved to Sent Items.
//!
//! <https://learn.microsoft.com/en-us/graph/api/message-send>

use alloc::{format, vec::Vec};

use io_http::rfc6750::bearer::HttpAuthBearer;
use log::{debug, trace};
use url::Url;

use crate::{
    coroutine::*,
    msgraph_try,
    v1::send::{
        MSGRAPH_API_BASE, MsgraphNoResponse, MsgraphSend, MsgraphSendError, MsgraphSendOutput,
        user_path,
    },
};

/// Sends an existing Microsoft Graph draft message.
pub struct MsgraphMessageSend {
    send: MsgraphSend<MsgraphNoResponse>,
}

impl MsgraphMessageSend {
    /// Sends the draft message `id`.
    pub fn new(auth: &HttpAuthBearer, user_id: &str, id: &str) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph draft send");
        trace!("id: {id:?}");

        let user = user_path(user_id);
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/messages/{id}/send"))?;
        let send = MsgraphSend::with_method(auth, "POST", url, None, Vec::new());

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphMessageSend {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphNoResponse>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("draft sent");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
