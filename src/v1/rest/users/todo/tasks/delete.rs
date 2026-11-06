//! Delete a Microsoft To Do task
//! (`DELETE /me/todo/lists/{id}/tasks/{id}`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/todotask-delete>

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

/// Deletes a Microsoft To Do task.
pub struct MsgraphTodoTaskDelete {
    send: MsgraphSend<MsgraphNoResponse>,
}

impl MsgraphTodoTaskDelete {
    /// Deletes the task `id` of the list `list_id`.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        list_id: &str,
        id: &str,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph to do task deletion");
        trace!("list: {list_id:?}, id: {id:?}");

        let user = user_path(user_id);
        let path = format!("{user}/todo/lists/{list_id}/tasks/{id}");
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&path)?;
        let send = MsgraphSend::delete(auth, url);

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphTodoTaskDelete {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphNoResponse>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("to do task deleted");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
