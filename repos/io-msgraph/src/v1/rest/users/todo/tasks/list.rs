//! List the tasks of a Microsoft To Do list
//! (`GET /me/todo/lists/{id}/tasks`).
//!
//! <https://learn.microsoft.com/en-us/graph/api/todotasklist-list-tasks>

use alloc::{format, string::String, vec::Vec};

use io_http::rfc6750::bearer::HttpAuthBearer;
use log::{debug, trace};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    coroutine::*,
    msgraph_try,
    v1::{
        query::to_query_pairs,
        rest::users::todo::tasks::MsgraphTodoTask,
        send::{MSGRAPH_API_BASE, MsgraphSend, MsgraphSendError, MsgraphSendOutput, user_path},
    },
};

/// One page of tasks (`value` plus the OData paging link).
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct MsgraphTodoTasksListResponse {
    /// The tasks of the page.
    #[serde(default)]
    pub value: Vec<MsgraphTodoTask>,
    /// The URL of the next page, when one exists.
    #[serde(default, rename = "@odata.nextLink")]
    pub next_link: Option<String>,
}

/// OData query parameters for listing tasks.
#[derive(Debug, Clone, Default, Serialize, Eq, PartialEq)]
pub struct MsgraphTodoTasksListParams<'a> {
    /// Maximum number of tasks per page (`$top`).
    #[serde(rename = "$top")]
    pub top: Option<u32>,
    /// Number of tasks to skip (`$skip`).
    #[serde(rename = "$skip")]
    pub skip: Option<u32>,
    /// Comma-separated properties to return (`$select`).
    #[serde(rename = "$select")]
    pub select: Option<&'a str>,
    /// OData filter expression (`$filter`).
    #[serde(rename = "$filter")]
    pub filter: Option<&'a str>,
    /// Comma-separated sort properties (`$orderby`).
    #[serde(rename = "$orderby")]
    pub orderby: Option<&'a str>,
    /// Navigation clause to expand (`$expand`).
    #[serde(rename = "$expand")]
    pub expand: Option<&'a str>,
}

/// Lists the tasks of a Microsoft To Do list.
pub struct MsgraphTodoTasksList {
    send: MsgraphSend<MsgraphTodoTasksListResponse>,
}

impl MsgraphTodoTasksList {
    /// Lists the tasks of the list `list_id`.
    pub fn new(
        auth: &HttpAuthBearer,
        user_id: &str,
        list_id: &str,
        params: &MsgraphTodoTasksListParams,
    ) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph to do tasks listing");
        trace!("list: {list_id:?}");
        trace!("params: {params:?}");

        let user = user_path(user_id);
        let path = format!("{user}/todo/lists/{list_id}/tasks");
        let mut url = Url::parse(MSGRAPH_API_BASE)?.join(&path)?;
        url.query_pairs_mut().extend_pairs(to_query_pairs(params));

        let send = MsgraphSend::get(auth, url);

        Ok(Self { send })
    }

    /// Continues a listing from an `@odata.nextLink`, sent as-is.
    pub fn from_link(auth: &HttpAuthBearer, link: &str) -> Result<Self, MsgraphSendError> {
        debug!("prepare microsoft graph to do tasks listing from link");
        trace!("link: {link}");

        let url = Url::parse(link)?;
        let send = MsgraphSend::get(auth, url);

        Ok(Self { send })
    }

    /// Asks Graph for at most `size` items per page
    /// (`Prefer: odata.maxpagesize={size}`), see
    /// [`MsgraphSend::max_page_size`]. Graph keeps no memory of it
    /// across pages: chain it on every page request, the ones built
    /// from an `@odata.nextLink` included.
    pub fn max_page_size(mut self, size: u32) -> Self {
        self.send = self.send.max_page_size(size);
        self
    }
}

impl MsgraphCoroutine for MsgraphTodoTasksList {
    type Yield = MsgraphYield;
    type Return = Result<MsgraphSendOutput<MsgraphTodoTasksListResponse>, MsgraphSendError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return> {
        let out = msgraph_try!(&mut self.send, arg);
        debug!("to do tasks listed");
        trace!("out: {out:?}");
        MsgraphCoroutineState::Complete(Ok(out))
    }
}
