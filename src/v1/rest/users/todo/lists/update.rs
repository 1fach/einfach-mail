//! Update a Microsoft To Do task list (`PATCH /me/todo/lists/{id}`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/todotasklist-update>

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

/// Updates a Microsoft To Do task list.
pub struct MsgraphTodoListUpdate {
    send: MsgraphSend<MsgraphTodoList>,
}

impl MsgraphTodoListUpdate {
    /// Patches the task list `id` with the set and null fields of `list`.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        id: &str,
        list: &MsgraphTodoList,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph to do list update");
        trace!("id: {id:?}");
        trace!("list: {list:?}");

        let user = user_path(user_id);
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&format!("{user}/todo/lists/{id}"))?;
        let send = MsgraphSend::patch_json(auth, url, list)?;

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphTodoListUpdate {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphTodoList>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("to do list updated");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
