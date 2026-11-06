//! Microsoft Graph mail folders (`users.mailFolders`): list, get,
//! create, update, delete, copy, move, list child folders.
//!
//! <https://learn.microsoft.com/en-us/graph/api/resources/mailfolder>

use alloc::string::String;

use serde::{Deserialize, Serialize};

pub mod child_folders;
pub mod copy;
pub mod create;
pub mod delete;
pub mod get;
pub mod list;
pub mod r#move;
pub mod update;

/// A mail folder in a user's mailbox. Doubles as the create body, where
/// only `display_name` (and optionally `is_hidden`) is serialized.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct MsgraphMailFolder {
    /// The unique identifier of the folder.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// The display name of the folder.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    /// The identifier of the parent folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_folder_id: Option<String>,
    /// The number of immediate child folders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_folder_count: Option<u64>,
    /// The number of unread items in the folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread_item_count: Option<u64>,
    /// The total number of items in the folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_item_count: Option<u64>,
    /// The size of the folder in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_in_bytes: Option<u64>,
    /// Whether the folder is hidden from folder listings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_hidden: Option<bool>,
}
