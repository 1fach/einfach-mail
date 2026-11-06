//! Update a Microsoft Graph calendar (`PATCH /me/calendars/{id}`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/calendar-update>

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

/// Updates a Microsoft Graph calendar.
pub struct MsgraphCalendarUpdate {
    send: MsgraphSend<MsgraphCalendar>,
}

impl MsgraphCalendarUpdate {
    /// Patches the calendar `id` with the set and null fields of `calendar`.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        id: &str,
        calendar: &MsgraphCalendar,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph calendar update");
        trace!("id: {id:?}");
        trace!("calendar: {calendar:?}");

        let user = user_path(user_id);
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/calendars/{id}"))?;
        let send = MsgraphSend::patch_json(auth, url, calendar)?;

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphCalendarUpdate {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphCalendar>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("calendar updated");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
