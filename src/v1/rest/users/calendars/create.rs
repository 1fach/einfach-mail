//! Create a Microsoft Graph calendar (`POST /me/calendars`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/user-post-calendars>

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

/// Creates a Microsoft Graph calendar.
pub struct MsgraphCalendarCreate {
    send: MsgraphSend<MsgraphCalendar>,
}

impl MsgraphCalendarCreate {
    /// Creates the calendar.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        calendar: &MsgraphCalendar,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph calendar creation");
        trace!("calendar: {calendar:?}");

        let user = user_path(user_id);
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/calendars"))?;
        let send = MsgraphSend::post_json(auth, url, calendar)?;

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphCalendarCreate {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphCalendar>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("calendar created");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
