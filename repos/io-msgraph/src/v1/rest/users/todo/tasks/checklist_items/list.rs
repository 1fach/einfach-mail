//! List the checklist items of a Microsoft To Do task
//! (`GET /me/todo/lists/{id}/tasks/{id}/checklistItems`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/todotask-list-checklistitems>

use alloc::{format, string::String, vec::Vec};

use io_http::rfc6750::bearer::HttpAuthBearer;
use log::{debug, trace};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    coroutine::*,
    msgraph_try,
    v1::{
        rest::users::todo::tasks::checklist_items::MsgraphChecklistItem,
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

/// One page of checklist items (`value` plus the OData paging link).
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct MsgraphChecklistItemsListResponse {
    /// The checklist items of the page.
    #[serde(default)]
    pub value: Vec<MsgraphChecklistItem>,
    /// The URL of the next page, when one exists.
    #[serde(default, rename = "@odata.nextLink")]
    pub next_link: Option<String>,
}

/// Lists the checklist items of a Microsoft To Do task.
pub struct MsgraphChecklistItemsList {
    send: MsgraphSend<MsgraphChecklistItemsListResponse>,
}

impl MsgraphChecklistItemsList {
    /// Lists the checklist items of the task `task_id` of the list
    /// `list_id`.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        list_id: &str,
        task_id: &str,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph checklist items listing");
        trace!("list: {list_id:?}, task: {task_id:?}");

        let user = user_path(user_id);
        let path = format!("{user}/todo/lists/{list_id}/tasks/{task_id}/checklistItems");
        let url = Url::parse(MSGRAPH_API_BASE)?.join(&path)?;
        let send = MsgraphSend::get(auth, url);

        Ok(Self { send })
    }
}

impl MsgraphCoroutine for MsgraphChecklistItemsList {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphChecklistItemsListResponse>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("checklist items listed");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
