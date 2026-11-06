//! Microsoft To Do task lists (`users.todo.lists`): list, get, create,
//! update, delete and delta.
//!
//! <https://learn.microsoft.com/en-us/graph/api/resources/todotasklist>

use alloc::string::String;

use serde::{Deserialize, Serialize};

use crate::v1::field::MsgraphField;

pub mod create;
pub mod delete;
pub mod delta;
pub mod get;
pub mod list;
pub mod update;

/// A To Do task list of the user (Graph's `todoTaskList`). Doubles as
/// the create/update body.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MsgraphTodoList {
    /// The version of the list, changing with every edit, read-only.
    #[serde(
        default,
        rename = "@odata.etag",
        skip_serializing_if = "Option::is_none"
    )]
    pub etag: Option<String>,
    /// The unique identifier of the list.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// The list name.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub display_name: MsgraphField<String>,
    /// Whether the user owns the list, read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_owner: Option<bool>,
    /// Whether the list is shared with other users, read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_shared: Option<bool>,
    /// Which built-in list this is, if any, read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wellknown_list_name: Option<MsgraphTodoWellknownListName>,
}

/// The built-in list a To Do list is, which cannot be renamed or
/// deleted.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum MsgraphTodoWellknownListName {
    /// A list the user created.
    None,
    /// The built-in Tasks list.
    DefaultList,
    /// The built-in Flagged email list, holding the tasks of flagged
    /// messages.
    FlaggedEmails,
    /// A built-in list this version does not know.
    #[serde(other)]
    UnknownFutureValue,
}
