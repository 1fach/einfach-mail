//! Copy a Microsoft Graph message to a folder
//! (`POST /me/messages/{id}/copy`); returns the new copy.
//!
//! <https://learn.microsoft.com/en-us/graph/api/message-copy>

use alloc::format;

use io_http::rfc6750::bearer::HttpAuthBearer;
use log::{debug, trace};
use serde::Serialize;
use url::Url;

use crate::{
    coroutine::*,
    msgraph_try,
    v1::{
        rest::users::messages::MsgraphMessage,
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MsgraphMessageCopyRequest<'a> {
    destination_id: &'a str,
}

/// Copies a Microsoft Graph message into another folder.
pub struct MsgraphMessageCopy {
    send: MsgraphSend<MsgraphMessage>,
}

impl MsgraphMessageCopy {
    /// Copies `id` into `destination` (a folder id or a well-known name
    /// such as `archive`).
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        id: &str,
        destination: &str,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph message copy");
        trace!("id: {id:?}");
        trace!("destination: {destination:?}");

        let user = user_path(user_id);
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/messages/{id}/copy"))?;
        let body = MsgraphMessageCopyRequest {
            destination_id: destination,
        };
        let send = MsgraphSend::post_json(auth, url, &body)?;

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphMessageCopy {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphMessage>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("message copied");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
