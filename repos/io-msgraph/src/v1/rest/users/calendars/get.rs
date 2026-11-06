//! Get a Microsoft Graph calendar (`GET /me/calendars/{id}`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/calendar-get>

use alloc::format;

use io_http::rfc6750::bearer::HttpAuthBearer;
use log::{debug, trace};
use url::Url;

use crate::{
    coroutine::*,
    msgraph_try,
    v1::{
        rest::users::calendars::MsgraphCalendar,
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

/// Gets a Microsoft Graph calendar.
pub struct MsgraphCalendarGet {
    send: MsgraphSend<MsgraphCalendar>,
}

impl MsgraphCalendarGet {
    /// Gets the calendar `id`.
    pub fn new(auth: &HttpAuthBearer, user_id: &str, id: &str) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph calendar retrieval");
        trace!("id: {id:?}");

        let user = user_path(user_id);
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/calendars/{id}"))?;
        let send = MsgraphSend::get(auth, url);

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphCalendarGet {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphCalendar>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("calendar retrieved");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
