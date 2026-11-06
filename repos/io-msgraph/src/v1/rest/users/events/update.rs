//! Update a Microsoft Graph event (`PATCH /me/events/{id}`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/event-update>

use alloc::format;

use io_http::rfc6750::bearer::HttpAuthBearer;
use log::{debug, trace};
use url::Url;

use crate::{
    coroutine::*,
    msgraph_try,
    v1::{
        rest::users::events::MsgraphEvent,
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

/// Updates a Microsoft Graph event.
pub struct MsgraphEventUpdate {
    send: MsgraphSend<MsgraphEvent>,
}

impl MsgraphEventUpdate {
    /// Patches the event `id` with the set and null fields of `event`.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        id: &str,
        event: &MsgraphEvent,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph event update");
        trace!("id: {id:?}");
        trace!("event: {event:?}");

        let user = user_path(user_id);
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/events/{id}"))?;
        let send = MsgraphSend::patch_json(auth, url, event)?;

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphEventUpdate {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphEvent>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("event updated");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
