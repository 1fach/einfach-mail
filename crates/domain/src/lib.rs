//! Provider-independent mail and calendar concepts.
//!
//! Keep Graph payloads, database rows, UI framework types, and operating-system APIs out of this crate.

mod account;
mod calendar;
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
pub use identifiers::{AccountId, CalendarId, EmptyIdentifier, EventId, FolderId, MessageId};
pub use mail::{
    FolderParentError, FolderReference, MailFolder, MailMessage, MessageBody, MessageBodyType,
    MessageDeltaChange, MessageDeltaLink, MessageDeltaPage, MessageDeltaProgress, MessageMetadata,
    MessageMetadataPatch, MessageOrigin, ReadState,
};
pub use mailbox::{EmptyMailboxAddress, Mailbox};
