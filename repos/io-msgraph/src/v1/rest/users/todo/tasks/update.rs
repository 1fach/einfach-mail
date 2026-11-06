//! Update a Microsoft To Do task
//! (`PATCH /me/todo/lists/{id}/tasks/{id}`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/todotask-update>

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

/// Updates a Microsoft To Do task.
pub struct MsgraphTodoTaskUpdate {
    send: MsgraphSend<MsgraphTodoTask>,
}

impl MsgraphTodoTaskUpdate {
    /// Patches the task `id` of the list `list_id` with the set and null
    /// fields of `task`; linked resources and checklist items are not
    /// written this way.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        list_id: &str,
        id: &str,
        task: &MsgraphTodoTask,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph to do task update");
        trace!("list: {list_id:?}, id: {id:?}");
        trace!("task: {task:?}");

        let user = user_path(user_id);
        let path = format!("{user}/todo/lists/{list_id}/tasks/{id}");
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&path)?;
        let send = MsgraphSend::patch_json(auth, url, task)?;

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphTodoTaskUpdate {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphTodoTask>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("to do task updated");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
