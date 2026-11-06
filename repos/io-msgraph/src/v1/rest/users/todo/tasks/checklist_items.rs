//! Microsoft To Do checklist items (`users.todo.lists.tasks.checklistItems`):
//! the steps of a task, listed.
//!
//! <https://learn.microsoft.com/en-us/graph/api/resources/checklistitem>

use alloc::string::String;

use serde::{Deserialize, Serialize};

pub mod list;

/// A step of a task.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MsgraphChecklistItem {
    /// The unique identifier of the item.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// The title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Whether the item is checked off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_checked: Option<bool>,
    /// When the item was checked off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_date_time: Option<String>,
    /// When the item was created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_date_time: Option<String>,
}
