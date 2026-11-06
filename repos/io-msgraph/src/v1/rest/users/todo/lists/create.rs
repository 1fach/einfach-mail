//! Create a Microsoft To Do task list (`POST /me/todo/lists`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/todo-post-lists>

use alloc::format;

use io_http::rfc6750::bearer::HttpAuthBearer;
use log::{debug, trace};
use url::Url;

use crate::{
    coroutine::*,
    msgraph_try,
    v1::{
        rest::users::todo::lists::MsgraphTodoList,
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

/// Creates a Microsoft To Do task list.
pub struct MsgraphTodoListCreate {
    send: MsgraphSend<MsgraphTodoList>,
}

impl MsgraphTodoListCreate {
    /// Creates the task list.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        list: &MsgraphTodoList,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph to do list creation");
        trace!("list: {list:?}");

        let user = user_path(user_id);
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/todo/lists"))?;
        let send = MsgraphSend::post_json(auth, url, list)?;

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphTodoListCreate {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphTodoList>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("to do list created");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
