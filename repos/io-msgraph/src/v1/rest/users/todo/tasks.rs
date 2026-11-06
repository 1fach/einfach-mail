//! Microsoft To Do tasks (`users.todo.lists.tasks`): list, get, create,
//! update, delete and delta, plus the checklist items of a task.
//!
//! A task lives in one list and is addressed through it. Its linked
//! resources (the message a flagged-email task came from, a link an app
//! attached) come inline with every read; they are created inline only
//! at create, and kept apart from the task afterwards.
//!
//! <https://learn.microsoft.com/en-us/graph/api/resources/todotask>

use alloc::{string::String, vec::Vec};

use serde::{Deserialize, Serialize};

use crate::v1::{
    field::MsgraphField,
    rest::users::{
        contacts::MsgraphSingleValueExtendedProperty,
        events::{MsgraphDateTimeTimeZone, MsgraphPatternedRecurrence},
        messages::{MsgraphImportance, MsgraphItemBody},
        todo::tasks::checklist_items::MsgraphChecklistItem,
    },
};

pub mod checklist_items;
pub mod create;
pub mod delete;
pub mod delta;
pub mod get;
#[cfg(feature = "ical")]
pub mod ical;
pub mod list;
pub mod update;

/// A To Do task (Graph's `todoTask`). Doubles as the create/update
/// body.
///
/// Unset fields are left out (an update preserves them), null fields are
/// serialized as explicit nulls (an update clears them), and a set empty
/// collection clears the collection. Graph keeps the due, start and
/// completion dates as midnights, in the zone they are sent in.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MsgraphTodoTask {
    /// The version of the task, changing with every edit, read-only.
    #[serde(
        default,
        rename = "@odata.etag",
        skip_serializing_if = "Option::is_none"
    )]
    pub etag: Option<String>,
    /// The unique identifier of the task, changing when it moves to
    /// another list.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// When the task was created, read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_date_time: Option<String>,
    /// When the task was last modified, read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified_date_time: Option<String>,
    /// When the body was last modified, read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_last_modified_date_time: Option<String>,
    /// The title.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub title: MsgraphField<String>,
    /// The body.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub body: MsgraphField<MsgraphItemBody>,
    /// The categories, any strings.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub categories: MsgraphField<Vec<String>>,
    /// The importance.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub importance: MsgraphField<MsgraphImportance>,
    /// The progress state.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub status: MsgraphField<MsgraphTodoTaskStatus>,
    /// The due date, a midnight in a zone.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub due_date_time: MsgraphField<MsgraphDateTimeTimeZone>,
    /// The start date, a midnight in a zone.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub start_date_time: MsgraphField<MsgraphDateTimeTimeZone>,
    /// The completion date, a midnight in a zone.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub completed_date_time: MsgraphField<MsgraphDateTimeTimeZone>,
    /// Whether the reminder is set.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub is_reminder_on: MsgraphField<bool>,
    /// When the reminder fires.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub reminder_date_time: MsgraphField<MsgraphDateTimeTimeZone>,
    /// The recurrence, repeating on the due date.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub recurrence: MsgraphField<MsgraphPatternedRecurrence>,
    /// Whether the task has file attachments, read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_attachments: Option<bool>,
    /// The linked resources, writable at create only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub linked_resources: Vec<MsgraphLinkedResource>,
    /// The checklist items, returned only when expanded, read-only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checklist_items: Vec<MsgraphChecklistItem>,
    /// Single-value extended properties, returned only when expanded.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub single_value_extended_properties: MsgraphField<Vec<MsgraphSingleValueExtendedProperty>>,
}

/// An item a task links to in another app.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MsgraphLinkedResource {
    /// The unique identifier of the linked resource.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// The deep link to the item, absent for some.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_url: Option<String>,
    /// The name of the app the item belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application_name: Option<String>,
    /// The title of the item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// The id of the item in its app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
}

/// The progress state of a task.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum MsgraphTodoTaskStatus {
    /// Not started.
    NotStarted,
    /// In progress.
    InProgress,
    /// Completed.
    Completed,
    /// Waiting on someone else.
    WaitingOnOthers,
    /// Deferred.
    Deferred,
}
