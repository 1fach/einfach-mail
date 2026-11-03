//! Provider-independent mail and calendar concepts.
//!
//! Keep Graph payloads, database rows, UI framework types, and operating-system APIs out of this crate.

mod account;
mod calendar;
mod calendar_creation;
mod identifiers;
mod mail;
mod mailbox;

pub use account::{AccountProfile, MissingAccountLabel};
pub use calendar::{
    CalendarDeltaChange, CalendarDeltaLink, CalendarDeltaPage, CalendarDeltaProgress,
    CalendarField, CalendarOccurrence, CalendarOccurrencePatch, CalendarOccurrenceTime,
    CalendarOccurrenceTimePatch, CalendarViewRange, CalendarViewReference,
    InvalidCalendarOccurrenceTime, InvalidCalendarViewRange, NewCalendarOccurrenceError,
};
pub use calendar_creation::{
    CalendarEventCreation, CalendarEventCreationId, CalendarEventDraft, CalendarTimeZoneId,
    InvalidCalendarEventCreation, InvalidCalendarEventCreationId, InvalidCalendarEventDraft,
    InvalidCalendarEventTimeZone, PendingCalendarEventCreation,
};
pub use identifiers::{AccountId, CalendarId, EmptyIdentifier, EventId, FolderId, MessageId};
pub use mail::{
    FolderParentError, FolderReference, FollowUpFlagStatus, InvalidMailRecipient,
    InvalidMailSubject, MailDraft, MailFolder, MailMessage, MailRecipient, MailReplyDraft,
    MailReplySource, MailSendOperationId, MailSendTarget, MailSubject, MessageBody,
    MessageBodyType, MessageDeltaChange, MessageDeltaLink, MessageDeltaPage, MessageDeltaProgress,
    MessageMetadata, MessageMetadataPatch, MessageOrigin, PendingMailSend, ReadState,
};
pub use mailbox::{EmptyMailboxAddress, Mailbox};
