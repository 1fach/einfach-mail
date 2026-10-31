//! Provider-independent mail and calendar concepts.
//!
//! Keep Graph payloads, database rows, UI framework types, and operating-system APIs out of this crate.

mod identifiers;

pub use identifiers::{AccountId, CalendarId, EmptyIdentifier, EventId, FolderId, MessageId};
