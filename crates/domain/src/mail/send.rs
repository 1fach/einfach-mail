//! Message drafts and body-free metadata for send attempts.

use std::{fmt, str::FromStr, time::SystemTime};

use addr_spec::AddrSpec;
use thiserror::Error;
use uuid::Uuid;

use super::FolderReference;
use crate::{AccountId, MessageId};

/// A syntactically valid, single recipient address.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct MailRecipient(Box<str>);

impl MailRecipient {
    /// Returns the validated recipient address.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for MailRecipient {
    type Err = InvalidMailRecipient;

    /// Parses one bare address, allowing surrounding whitespace.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidMailRecipient`] when the value is not one strict
    /// RFC 5322 address without display-name syntax.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.trim();
        let address = value
            .parse::<AddrSpec>()
            .map_err(|_| InvalidMailRecipient)?;
        Ok(Self(address.to_string().into()))
    }
}

impl fmt::Debug for MailRecipient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MailRecipient(<redacted>)")
    }
}

/// The recipient is not one strict bare mail address.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("recipient must be one valid bare mail address")]
#[must_use]
pub struct InvalidMailRecipient;

/// A non-empty, single-line mail subject.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct MailSubject(Box<str>);

impl MailSubject {
    /// Returns the validated subject text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for MailSubject {
    type Err = InvalidMailSubject;

    /// Parses a subject after trimming surrounding whitespace.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidMailSubject::Empty`] for an empty value or
    /// [`InvalidMailSubject::Multiline`] when it contains a line break.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.trim();
        if value.is_empty() {
            return Err(InvalidMailSubject::Empty);
        }
        if value.contains(['\r', '\n']) {
            return Err(InvalidMailSubject::Multiline);
        }
        Ok(Self(value.into()))
    }
}

impl fmt::Debug for MailSubject {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MailSubject(<redacted>)")
    }
}

/// A subject value that cannot be sent as a single mail header line.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[must_use]
pub enum InvalidMailSubject {
    /// The subject is empty after trimming surrounding whitespace.
    #[error("the message subject is empty")]
    Empty,
    /// The subject contains a line break.
    #[error("the message subject contains a line break")]
    Multiline,
}

/// The account-scoped cached message that determines a reply's recipients.
#[derive(Clone, Eq, PartialEq)]
pub struct MailReplySource {
    folder: FolderReference,
    message_id: MessageId,
}

impl MailReplySource {
    /// Creates a source reference from its cached folder and message IDs.
    #[must_use]
    pub fn new(folder: FolderReference, message_id: MessageId) -> Self {
        Self { folder, message_id }
    }

    /// Returns the account that owns the source message.
    #[must_use]
    pub fn account_id(&self) -> &AccountId {
        self.folder.account_id()
    }

    /// Returns the cached folder containing the source message.
    #[must_use]
    pub fn folder(&self) -> &FolderReference {
        &self.folder
    }

    /// Returns the provider-issued ID of the source message.
    #[must_use]
    pub fn message_id(&self) -> &MessageId {
        &self.message_id
    }
}

impl fmt::Debug for MailReplySource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MailReplySource(<redacted>)")
    }
}

/// An in-memory plain-text reply draft for a cached message.
#[derive(Eq, PartialEq)]
pub struct MailReplyDraft {
    source: MailReplySource,
    body: Box<str>,
}

impl MailReplyDraft {
    /// Creates a reply draft with the source message and plain-text body.
    #[must_use]
    pub fn new(source: MailReplySource, body: String) -> Self {
        Self {
            source,
            body: body.into_boxed_str(),
        }
    }

    /// Returns the source whose reply recipients Graph will use.
    #[must_use]
    pub fn source(&self) -> &MailReplySource {
        &self.source
    }

    /// Returns the plain-text reply body.
    #[must_use]
    pub fn body(&self) -> &str {
        &self.body
    }

    /// Creates the body-free operation marker saved before submission.
    #[must_use]
    pub fn pending_send(
        &self,
        operation_id: MailSendOperationId,
        source_subject: Option<&str>,
        created_at: SystemTime,
    ) -> PendingMailSend {
        PendingMailSend::for_reply(
            self.source.clone(),
            operation_id,
            source_subject.map(Into::into),
            created_at,
        )
    }
}

impl fmt::Debug for MailReplyDraft {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MailReplyDraft")
            .field("source", &"<redacted>")
            .field("body", &"<redacted>")
            .finish()
    }
}

/// An in-memory plain-text message ready for explicit submission.
#[derive(Eq, PartialEq)]
pub struct MailDraft {
    recipient: MailRecipient,
    subject: MailSubject,
    body: Box<str>,
}

impl MailDraft {
    /// Creates a message draft with a validated recipient and subject.
    #[must_use]
    pub fn new(recipient: MailRecipient, subject: MailSubject, body: String) -> Self {
        Self {
            recipient,
            subject,
            body: body.into_boxed_str(),
        }
    }

    /// Returns the recipient address.
    #[must_use]
    pub fn recipient(&self) -> &MailRecipient {
        &self.recipient
    }

    /// Returns the subject.
    #[must_use]
    pub fn subject(&self) -> &str {
        self.subject.as_str()
    }

    /// Returns the plain-text message body.
    #[must_use]
    pub fn body(&self) -> &str {
        &self.body
    }

    /// Creates the body-free operation marker saved before submission.
    #[must_use]
    pub fn pending_send(
        &self,
        account_id: AccountId,
        operation_id: MailSendOperationId,
        created_at: SystemTime,
    ) -> PendingMailSend {
        PendingMailSend::for_new_message(
            account_id,
            operation_id,
            self.recipient.clone(),
            self.subject.clone(),
            created_at,
        )
    }
}

impl fmt::Debug for MailDraft {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MailDraft")
            .field("recipient", &"<redacted>")
            .field("subject", &"<redacted>")
            .field("body", &"<redacted>")
            .finish()
    }
}

/// A local identifier for one send attempt, not a Graph idempotency key.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct MailSendOperationId(Uuid);

impl MailSendOperationId {
    /// Creates a random identifier for one local send attempt.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Restores the identifier from a UUID validated by the persistence boundary.
    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    /// Returns the UUID used by the local cache.
    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for MailSendOperationId {
    fn default() -> Self {
        Self::new()
    }
}

/// An unresolved send marker encrypted in the account's local cache.
#[derive(Clone, Eq, PartialEq)]
pub struct PendingMailSend {
    account_id: AccountId,
    operation_id: MailSendOperationId,
    target: MailSendTarget,
    created_at: SystemTime,
}

/// The body-free target details retained for one unresolved send.
#[derive(Clone, Eq, PartialEq)]
pub enum MailSendTarget {
    /// A new message addressed to one recipient.
    NewMessage {
        /// The recipient retained for reconciliation.
        recipient: MailRecipient,
        /// The subject retained for reconciliation.
        subject: MailSubject,
    },
    /// A reply tied to its original cached message.
    Reply {
        /// The original message whose reply recipients Graph will select.
        source: MailReplySource,
        /// The original subject, when available.
        subject: Option<Box<str>>,
    },
}

impl MailSendTarget {
    /// Returns the new-message recipient, when this operation is not a reply.
    #[must_use]
    pub fn recipient(&self) -> Option<&MailRecipient> {
        match self {
            Self::NewMessage { recipient, .. } => Some(recipient),
            Self::Reply { .. } => None,
        }
    }

    /// Returns the retained subject, when one is available.
    #[must_use]
    pub fn subject(&self) -> Option<&str> {
        match self {
            Self::NewMessage { subject, .. } => Some(subject.as_str()),
            Self::Reply { subject, .. } => subject.as_deref(),
        }
    }

    /// Returns the source message, when this operation is a reply.
    #[must_use]
    pub fn reply_source(&self) -> Option<&MailReplySource> {
        match self {
            Self::NewMessage { .. } => None,
            Self::Reply { source, .. } => Some(source),
        }
    }
}

impl fmt::Debug for MailSendTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NewMessage { .. } => "MailSendTarget::NewMessage(<redacted>)",
            Self::Reply { .. } => "MailSendTarget::Reply(<redacted>)",
        })
    }
}

impl PendingMailSend {
    /// Creates body-free metadata for a new-message send.
    #[must_use]
    pub fn for_new_message(
        account_id: AccountId,
        operation_id: MailSendOperationId,
        recipient: MailRecipient,
        subject: MailSubject,
        created_at: SystemTime,
    ) -> Self {
        Self {
            account_id,
            operation_id,
            target: MailSendTarget::NewMessage { recipient, subject },
            created_at,
        }
    }

    /// Creates body-free metadata for a reply send.
    #[must_use]
    pub fn for_reply(
        source: MailReplySource,
        operation_id: MailSendOperationId,
        subject: Option<Box<str>>,
        created_at: SystemTime,
    ) -> Self {
        Self {
            account_id: source.account_id().clone(),
            operation_id,
            target: MailSendTarget::Reply { source, subject },
            created_at,
        }
    }

    /// Returns the account that owns this operation.
    #[must_use]
    pub fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns the local identifier for this operation.
    #[must_use]
    pub const fn operation_id(&self) -> MailSendOperationId {
        self.operation_id
    }

    /// Returns the target details kept for reconciliation.
    #[must_use]
    pub fn target(&self) -> &MailSendTarget {
        &self.target
    }

    /// Returns when the app staged this operation.
    #[must_use]
    pub fn created_at(&self) -> SystemTime {
        self.created_at
    }
}

impl fmt::Debug for PendingMailSend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingMailSend")
            .field("account_id", &self.account_id)
            .field("operation_id", &self.operation_id)
            .field("recipient", &"<redacted>")
            .field("subject", &"<redacted>")
            .field("created_at", &self.created_at)
            .finish()
    }
}

#[cfg(test)]
mod recipient_tests;
