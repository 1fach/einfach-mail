//! Get a Microsoft To Do task (`GET /me/todo/lists/{id}/tasks/{id}`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/todotask-get>

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

/// Gets a Microsoft To Do task.
pub struct MsgraphTodoTaskGet {
    send: MsgraphSend<MsgraphTodoTask>,
}

impl MsgraphTodoTaskGet {
    /// Gets the task `id` of the list `list_id`, trimmed to the
    /// `$select`ed properties and `$expand`ing the given navigation
    /// clause when either is passed.
    ///
    /// Extended properties and checklist items come back only expanded.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        list_id: &str,
        id: &str,
        select: Option<&str>,
        expand: Option<&str>,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph to do task retrieval");
        trace!("list: {list_id:?}, id: {id:?}");
        trace!("select: {select:?}, expand: {expand:?}");

        let user = user_path(user_id);
        let path = format!("{user}/todo/lists/{list_id}/tasks/{id}");
        let mut url = Url::parse(MSGRAPH_API_BASE)?.join(&path)?;

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

impl MsgraphCoroutine for MsgraphTodoTaskGet {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphTodoTask>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("to do task retrieved");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
