//! List the instances of a Microsoft Graph series
//! (`GET /me/events/{id}/instances`).
//!
//! The occurrences and exceptions of a series master within a window,
//! cancelled occurrences left out.
//!
//! <https://learn.microsoft.com/en-us/graph/api/event-list-instances>

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

/// Lists the instances of a Microsoft Graph series.
pub struct MsgraphEventInstances {
    send: MsgraphSend<MsgraphEventsListResponse>,
}

impl MsgraphEventInstances {
    /// Lists the instances of the series master `id` between `start` and
    /// `end`, ISO 8601 date-times.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        id: &str,
        start: &str,
        end: &str,
        params: &MsgraphEventsListParams,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph event instances listing");
        trace!("id: {id:?}, start: {start}, end: {end}");

        let user = user_path(user_id);
        let mut url =
            Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/events/{id}/instances"))?;
        url.query_pairs_mut()
            .append_pair("startDateTime", start)
            .append_pair("endDateTime", end)
            .extend_pairs(to_query_pairs(params));

        let send = MsgraphSend::get(auth, url);

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphEventInstances {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphEventsListResponse>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("event instances listed");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
