//! Get a Microsoft Graph event (`GET /me/events/{id}`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/event-get>

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

/// Gets a Microsoft Graph event.
pub struct MsgraphEventGet {
    send: MsgraphSend<MsgraphEvent>,
}

impl MsgraphEventGet {
    /// Gets the event `id`, trimmed to the `$select`ed properties and
    /// `$expand`ing the given navigation clause when either is passed.
    ///
    /// Extended properties come back only expanded, and a series'
    /// `cancelledOccurrences` only selected.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        id: &str,
        select: Option<&str>,
        expand: Option<&str>,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph event retrieval");
        trace!("id: {id:?}");
        trace!("select: {select:?}, expand: {expand:?}");

        let user = user_path(user_id);
        let mut url = Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/events/{id}"))?;

        if let Some(select) = select {
            url.query_pairs_mut().append_pair("$select", select);
        }
        if let Some(expand) = expand {
            url.query_pairs_mut().append_pair("$expand", expand);
        }

        let send = MsgraphSend::get(auth, url);

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphEventGet {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphEvent>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("event retrieved");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
