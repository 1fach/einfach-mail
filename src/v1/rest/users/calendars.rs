//! Microsoft Graph calendars (`users.calendars`): list, get, create,
//! update, delete.
//!
//! <https://learn.microsoft.com/en-us/graph/api/resources/calendar>

use alloc::string::String;

use serde::{Deserialize, Serialize};

use crate::v1::{field::MsgraphField, rest::users::messages::MsgraphEmailAddress};

pub mod create;
pub mod delete;
pub mod get;
pub mod list;
pub mod update;

/// A calendar of the user. Doubles as the create/update body.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MsgraphCalendar {
    /// The unique identifier of the calendar.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// The calendar name.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub name: MsgraphField<String>,
    /// The theme colour (`auto`, `lightBlue`, `lightGreen`...).
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub color: MsgraphField<String>,
    /// The colour as `#RRGGBB`, read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hex_color: Option<String>,
    /// The version of the calendar, changing with every edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_key: Option<String>,
    /// Whether the user can write to the calendar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_edit: Option<bool>,
    /// Whether this is the user's default calendar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_default_calendar: Option<bool>,
    /// Whether the calendar can be deleted from the user's mailbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_removable: Option<bool>,
    /// The owner of a shared calendar, or the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<MsgraphEmailAddress>,
}
