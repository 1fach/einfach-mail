//! Provider-independent mail and calendar concepts.
//!
//! Keep Graph payloads, database rows, UI framework types, and operating-system APIs out of this crate.

mod account;
mod identifiers;
mod mailbox;

pub use account::{AccountProfile, MissingAccountLabel};
pub use identifiers::{AccountId, CalendarId, EmptyIdentifier, EventId, FolderId, MessageId};
pub use mailbox::{EmptyMailboxAddress, Mailbox};
