//! Delete a Microsoft To Do task list (`DELETE /me/todo/lists/{id}`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/todotasklist-delete>

use alloc::format;

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

/// Deletes a Microsoft To Do task list.
pub struct MsgraphTodoListDelete {
    send: MsgraphSend<MsgraphNoResponse>,
}

impl MsgraphTodoListDelete {
    /// Deletes the task list `id` and its tasks.
    pub fn new(auth: &HttpAuthBearer, user_id: &str, id: &str) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph to do list deletion");
        trace!("id: {id:?}");

        let user = user_path(user_id);
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/todo/lists/{id}"))?;
        let send = MsgraphSend::delete(auth, url);

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphTodoListDelete {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphNoResponse>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("to do list deleted");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
