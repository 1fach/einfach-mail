//! Microsoft Graph user resource (`users`, or the `me` shortcut),
//! including its mail folders, messages, the sendMail action, contact
//! folders, contacts, calendars, events and To Do task lists.
//!
//! <https://learn.microsoft.com/en-us/graph/api/resources/user>

use alloc::string::String;

use serde::{Deserialize, Serialize};

pub mod calendars;
pub mod contact_folders;
pub mod contacts;
pub mod events;
pub mod get;
pub mod mail_folders;
pub mod messages;
pub mod send_mail;
pub mod todo;

/// A Microsoft Graph user (the signed-in mailbox owner via `me`).
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MsgraphUser {
    /// The unique identifier of the user.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// The display name of the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// The primary email address of the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mail: Option<String>,
    /// The principal name of the user, usually its sign-in address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_principal_name: Option<String>,
}
