//! Get a Microsoft To Do task list (`GET /me/todo/lists/{id}`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/todotasklist-get>

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

/// Gets a Microsoft To Do task list.
pub struct MsgraphTodoListGet {
    send: MsgraphSend<MsgraphTodoList>,
}

impl MsgraphTodoListGet {
    /// Gets the task list `id`.
    pub fn new(auth: &HttpAuthBearer, user_id: &str, id: &str) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph to do list retrieval");
        trace!("id: {id:?}");

        let user = user_path(user_id);
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/todo/lists/{id}"))?;
        let send = MsgraphSend::get(auth, url);

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphTodoListGet {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphTodoList>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("to do list retrieved");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
