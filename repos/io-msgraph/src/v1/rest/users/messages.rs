//! Microsoft Graph messages (`users.messages`): list, delta, get (JSON
//! and raw MIME), create draft (JSON and raw MIME), update, delete,
//! move, copy, send and the nested attachments collection.
//!
//! <https://learn.microsoft.com/en-us/graph/api/resources/message>

use alloc::{string::String, vec::Vec};

use serde::{Deserialize, Serialize};

use crate::v1::rest::users::contacts::MsgraphSingleValueExtendedProperty;

pub mod attachments;
pub mod copy;
pub mod create;
pub mod create_mime;
pub mod delete;
pub mod delta;
pub mod get;
pub mod get_raw;
pub mod list;
pub mod r#move;
pub mod send;
pub mod update;

/// A message in a mail folder. Doubles as the create/update body, where
/// only the set (non-empty) fields are serialized.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct MsgraphMessage {
    /// The unique identifier of the message.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// The version of the message, changing with every edit, read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_key: Option<String>,
    /// The subject of the message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// The first bytes of the body, in text format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_preview: Option<String>,
    /// The body of the message, in text or HTML format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<MsgraphItemBody>,
    /// The mailbox owner displayed as the author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<MsgraphRecipient>,
    /// The account actually used to send the message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<MsgraphRecipient>,
    /// The To recipients of the message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub to_recipients: Vec<MsgraphRecipient>,
    /// The Cc recipients of the message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cc_recipients: Vec<MsgraphRecipient>,
    /// The Bcc recipients of the message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bcc_recipients: Vec<MsgraphRecipient>,
    /// The addresses replies should be sent to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reply_to: Vec<MsgraphRecipient>,
    /// The reception date, as an ISO 8601 date-time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_date_time: Option<String>,
    /// The send date, as an ISO 8601 date-time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_date_time: Option<String>,
    /// Whether the message has been read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_read: Option<bool>,
    /// Whether the message is a draft.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_draft: Option<bool>,
    /// Whether the message carries attachments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_attachments: Option<bool>,
    /// The RFC 5322 Message-ID header of the message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub internet_message_id: Option<String>,
    /// The importance of the message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub importance: Option<MsgraphImportance>,
    /// The follow-up flag set on the message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<MsgraphFollowupFlag>,
    /// The categories associated with the message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    /// The identifier of the containing mail folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_folder_id: Option<String>,
    /// The identifier of the conversation the message belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// Single-value extended MAPI properties, returned only when the
    /// request `$expand`ed them by id (such as `Integer 0x0E08`, the
    /// message size).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub single_value_extended_properties: Vec<MsgraphSingleValueExtendedProperty>,
}

/// A message recipient, wrapping an email address.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct MsgraphRecipient {
    /// The email address of the recipient.
    pub email_address: MsgraphEmailAddress,
}

/// A named email address (`recipient.emailAddress`).
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct MsgraphEmailAddress {
    /// The display name paired with the address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The email address itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

/// The body of a message, in text or HTML format.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct MsgraphItemBody {
    /// The format of the body content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<MsgraphBodyType>,
    /// The body content itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// Follow-up flag set on a message.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct MsgraphFollowupFlag {
    /// The status of the follow-up flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag_status: Option<MsgraphFlagStatus>,
}

/// Format of a message body.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum MsgraphBodyType {
    /// Plain text body.
    Text,
    /// HTML body.
    Html,
}

/// Status of a follow-up flag.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum MsgraphFlagStatus {
    /// The message is not flagged.
    NotFlagged,
    /// The follow-up is completed.
    Complete,
    /// The message is flagged for follow-up.
    Flagged,
}

/// Importance of a message.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum MsgraphImportance {
    /// Low importance.
    Low,
    /// Normal importance, the default.
    Normal,
    /// High importance.
    High,
}
