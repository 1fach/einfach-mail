//! Provider-independent mail and calendar concepts.
//!
//! Keep Graph payloads, database rows, UI framework types, and operating-system APIs out of this crate.

mod account;
mod calendar;
mod mail;
mod shared;

use calendar::creation as calendar_creation;
use calendar::edit as calendar_edit;
use mail::mailbox;
use shared::identifiers;

pub use account::{
    AccountCapabilities, AccountProfile, CalendarAction, CalendarCapabilities, MailAction,
    MailCapabilities, MissingAccountLabel,
};
pub use calendar::{
    CalendarDeltaChange, CalendarDeltaLink, CalendarDeltaPage, CalendarDeltaProgress,
    CalendarOccurrence, CalendarOccurrencePatch, CalendarOccurrenceTime,
    CalendarOccurrenceTimePatch, CalendarViewRange, CalendarViewReference,
    InvalidCalendarOccurrenceTime, InvalidCalendarViewRange, NewCalendarOccurrenceError,
};
pub use calendar_creation::{
    CalendarEventCreation, CalendarEventCreationId, CalendarEventDraft, CalendarTimeZoneId,
    InvalidCalendarEventCreation, InvalidCalendarEventCreationId, InvalidCalendarEventDraft,
    InvalidCalendarEventTimeZone, PendingCalendarEventCreation,
};
pub use calendar_edit::{
    CalendarEventUpdate, CalendarEventUpdateDraft, CalendarEventVersion, EditableCalendarEvent,
    InvalidCalendarEventUpdateDraft, InvalidCalendarEventVersion, InvalidEditableCalendarEvent,
    NoCalendarEventChanges,
};
pub use identifiers::{AccountId, EmptyIdentifier, EventId, FolderId, MessageId};
pub use mail::{
    FolderParentError, FolderReference, FollowUpFlagStatus, InvalidMailRecipient,
    InvalidMailSubject, MailDraft, MailFolder, MailMessage, MailRecipient, MailReplyDraft,
    MailReplySource, MailSendOperationId, MailSendTarget, MailSubject, MessageBody,
    MessageBodyType, MessageDeltaChange, MessageDeltaLink, MessageDeltaPage, MessageDeltaProgress,
    MessageMetadata, MessageMetadataPatch, MessageOrigin, PendingMailSend, ReadState,
};
pub use mailbox::{EmptyMailboxAddress, Mailbox};
