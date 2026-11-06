//! List the Microsoft Graph calendar view (`GET /me/calendarView` or
//! `GET /me/calendars/{id}/calendarView`).
//!
//! Every lone event, occurrence and exception within a window, series
//! expanded.
//!
//! <https://learn.microsoft.com/en-us/graph/api/user-list-calendarview>

use alloc::format;

use io_http::rfc6750::bearer::HttpAuthBearer;
use log::{debug, trace};
use url::Url;

use crate::{
    coroutine::*,
    msgraph_try,
    v1::{
        query::to_query_pairs,
        rest::users::events::list::{MsgraphEventsListParams, MsgraphEventsListResponse},
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

/// Lists a Microsoft Graph calendar view.
pub struct MsgraphCalendarView {
    send: MsgraphSend<MsgraphEventsListResponse>,
}

impl MsgraphCalendarView {
    /// Lists the view of the default calendar, or of `calendar` when given,
    /// between `start` and `end`, ISO 8601 date-times.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        calendar: Option<&str>,
        start: &str,
        end: &str,
        params: &MsgraphEventsListParams,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph calendar view");
        trace!("calendar: {calendar:?}, start: {start}, end: {end}");

        let user = user_path(user_id);
        let path = match calendar {
            Some(calendar) => format!("{user}/calendars/{calendar}/calendarView"),
            None => format!("{user}/calendarView"),
        };
        let mut url = Url::parse(MSGRAPH_API_BASE)?.join(&path)?;
        url.query_pairs_mut()
            .append_pair("startDateTime", start)
            .append_pair("endDateTime", end)
            .extend_pairs(to_query_pairs(params));

        let send = MsgraphSend::get(auth, url);

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphCalendarView {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphEventsListResponse>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("calendar view listed");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
