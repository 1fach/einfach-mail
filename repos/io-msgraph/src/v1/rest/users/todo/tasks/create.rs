//! Create a Microsoft To Do task (`POST /me/todo/lists/{id}/tasks`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/todotasklist-post-tasks>

use alloc::format;

use io_http::rfc6750::bearer::HttpAuthBearer;
use log::{debug, trace};
use url::Url;

use crate::{
    coroutine::*,
    msgraph_try,
    v1::{
        rest::users::todo::tasks::MsgraphTodoTask,
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

/// Creates a Microsoft To Do task.
pub struct MsgraphTodoTaskCreate {
    send: MsgraphSend<MsgraphTodoTask>,
}

impl MsgraphTodoTaskCreate {
    /// Creates the task in the list `list_id`, its linked resources and
    /// extended properties included.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        list_id: &str,
        task: &MsgraphTodoTask,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph to do task creation");
        trace!("list: {list_id:?}");
        trace!("task: {task:?}");

        let user = user_path(user_id);
        let path = format!("{user}/todo/lists/{list_id}/tasks");
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&path)?;
        let send = MsgraphSend::post_json(auth, url, task)?;

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphTodoTaskCreate {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphTodoTask>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("to do task created");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
