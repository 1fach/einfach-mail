//! Provider-independent values for editing a standalone timed calendar event.

use std::fmt;

use thiserror::Error;
use time::OffsetDateTime;

use crate::{
    CalendarOccurrence, CalendarOccurrenceTime, calendar_subject::normalized_event_subject,
};

/// The provider version captured when an event is opened for editing.
///
/// Values are limited to 4,096 UTF-8 bytes and cannot contain control characters.
#[derive(Clone, Eq, PartialEq)]
pub struct CalendarEventVersion(Box<str>);

impl fmt::Debug for CalendarEventVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CalendarEventVersion(<redacted>)")
    }
}

impl TryFrom<String> for CalendarEventVersion {
    type Error = InvalidCalendarEventVersion;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
            return Err(InvalidCalendarEventVersion);
        }
        Ok(Self(value.into_boxed_str()))
    }
}

impl CalendarEventVersion {
    /// Returns the opaque provider version without normalization.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A provider version was missing or malformed.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("calendar event version must be bounded and control-free")]
pub struct InvalidCalendarEventVersion;

/// A timed event that the provider confirmed is safe to edit.
#[derive(Clone, Eq, PartialEq)]
pub struct EditableCalendarEvent {
    occurrence: CalendarOccurrence,
    version: CalendarEventVersion,
}

impl fmt::Debug for EditableCalendarEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EditableCalendarEvent(<redacted>)")
    }
}

impl EditableCalendarEvent {
    /// Creates an edit snapshot for a provider-validated timed event.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidEditableCalendarEvent`] when the occurrence is all-day.
    pub fn new(
        occurrence: CalendarOccurrence,
        version: CalendarEventVersion,
    ) -> Result<Self, InvalidEditableCalendarEvent> {
        if !matches!(occurrence.time(), CalendarOccurrenceTime::Timed { .. }) {
            return Err(InvalidEditableCalendarEvent);
        }
        Ok(Self {
            occurrence,
            version,
        })
    }

    /// Returns the provider-validated event values captured for editing.
    #[must_use]
    pub fn occurrence(&self) -> &CalendarOccurrence {
        &self.occurrence
    }

    /// Returns the provider version captured with the event values.
    #[must_use]
    pub fn version(&self) -> &CalendarEventVersion {
        &self.version
    }
}

/// An edit snapshot contains an all-day event.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("editable calendar event must use timed values")]
pub struct InvalidEditableCalendarEvent;

/// The changed subject and timed range submitted for an event update.
#[derive(Clone, Eq, PartialEq)]
pub struct CalendarEventUpdateDraft {
    subject: Box<str>,
    time: CalendarOccurrenceTime,
}

impl fmt::Debug for CalendarEventUpdateDraft {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CalendarEventUpdateDraft(<redacted>)")
    }
}

impl CalendarEventUpdateDraft {
    /// Creates a non-empty subject and ordered UTC timed range.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidCalendarEventUpdateDraft`] for a blank subject, control
    /// characters, an invalid range, or a non-UTC endpoint.
    pub fn timed(
        subject: String,
        start: OffsetDateTime,
        end: OffsetDateTime,
    ) -> Result<Self, InvalidCalendarEventUpdateDraft> {
        let subject = normalized_event_subject(&subject).ok_or(InvalidCalendarEventUpdateDraft)?;
        let time = CalendarOccurrenceTime::timed(start, end)
            .map_err(|_| InvalidCalendarEventUpdateDraft)?;
        Ok(Self {
            subject: subject.into(),
            time,
        })
    }

    /// Returns the requested subject.
    #[must_use]
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// Returns the requested UTC time range.
    #[must_use]
    pub fn time(&self) -> &CalendarOccurrenceTime {
        &self.time
    }
}

/// An event update contains an invalid subject or time range.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("calendar event update must contain a title and ordered UTC times")]
pub struct InvalidCalendarEventUpdateDraft;

/// A requested event update bound to the provider version used by the editor.
#[derive(Clone, Eq, PartialEq)]
pub struct CalendarEventUpdate {
    source: EditableCalendarEvent,
    draft: CalendarEventUpdateDraft,
}

impl fmt::Debug for CalendarEventUpdate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CalendarEventUpdate(<redacted>)")
    }
}

impl CalendarEventUpdate {
    /// Creates an update when the subject or timed range changed.
    ///
    /// # Errors
    ///
    /// Returns [`NoCalendarEventChanges`] when the requested values equal the
    /// captured event.
    pub fn new(
        source: EditableCalendarEvent,
        draft: CalendarEventUpdateDraft,
    ) -> Result<Self, NoCalendarEventChanges> {
        let occurrence = source.occurrence();
        if occurrence.subject() == Some(draft.subject()) && occurrence.time() == draft.time() {
            return Err(NoCalendarEventChanges);
        }
        Ok(Self { source, draft })
    }

    /// Returns the provider-validated event and version being changed.
    #[must_use]
    pub fn source(&self) -> &EditableCalendarEvent {
        &self.source
    }

    /// Returns the requested event values.
    #[must_use]
    pub fn draft(&self) -> &CalendarEventUpdateDraft {
        &self.draft
    }

    /// Reports whether `event` confirms this update for the captured event.
    #[must_use]
    pub fn confirms(&self, event: &CalendarOccurrence) -> bool {
        event.reference() == self.source.occurrence().reference()
            && event.id() == self.source.occurrence().id()
            && event.subject() == Some(self.draft.subject())
            && event.time() == self.draft.time()
    }
}

/// The requested values leave an event unchanged.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("calendar event update must change at least one value")]
pub struct NoCalendarEventChanges;

#[cfg(test)]
#[path = "edit/tests.rs"]
mod tests;
