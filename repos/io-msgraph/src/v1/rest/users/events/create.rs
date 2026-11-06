//! Create a Microsoft Graph event (`POST /me/events` or
//! `POST /me/calendars/{id}/events`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/user-post-events>

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

/// Creates a Microsoft Graph event.
pub struct MsgraphEventCreate {
    send: MsgraphSend<MsgraphEvent>,
}

impl MsgraphEventCreate {
    /// Creates the event in the default calendar, or in `calendar` when
    /// given (a calendar id).
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        calendar: Option<&str>,
        event: &MsgraphEvent,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph event creation");
        trace!("calendar: {calendar:?}");
        trace!("event: {event:?}");

        let user = user_path(user_id);
        let path = match calendar {
            Some(calendar) => format!("{user}/calendars/{calendar}/events"),
            None => format!("{user}/events"),
        };
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&path)?;
        let send = MsgraphSend::post_json(auth, url, event)?;

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphEventCreate {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphEvent>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("event created");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
