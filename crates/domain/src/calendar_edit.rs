//! Provider-independent values for editing a standalone timed calendar event.

use std::fmt;

use time::OffsetDateTime;

use crate::{CalendarOccurrence, CalendarOccurrenceTime};

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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidCalendarEventVersion;

impl fmt::Display for InvalidCalendarEventVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("calendar event version must be bounded and control-free")
    }
}

impl std::error::Error for InvalidCalendarEventVersion {}

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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidEditableCalendarEvent;

impl fmt::Display for InvalidEditableCalendarEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("editable calendar event must use timed values")
    }
}

impl std::error::Error for InvalidEditableCalendarEvent {}

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
        let subject = subject.trim();
        if subject.is_empty() || subject.chars().any(char::is_control) {
            return Err(InvalidCalendarEventUpdateDraft);
        }
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidCalendarEventUpdateDraft;

impl fmt::Display for InvalidCalendarEventUpdateDraft {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("calendar event update must contain a title and ordered UTC times")
    }
}

impl std::error::Error for InvalidCalendarEventUpdateDraft {}

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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NoCalendarEventChanges;

impl fmt::Display for NoCalendarEventChanges {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("calendar event update must change at least one value")
    }
}

impl std::error::Error for NoCalendarEventChanges {}

#[cfg(test)]
mod tests {
    use time::{Date, Month, OffsetDateTime};

    use crate::{
        AccountId, CalendarOccurrencePatch, CalendarViewRange, CalendarViewReference, EventId,
    };

    use super::{
        CalendarEventUpdate, CalendarEventUpdateDraft, CalendarEventVersion, EditableCalendarEvent,
        InvalidCalendarEventVersion, NoCalendarEventChanges,
    };

    #[test]
    fn validates_event_versions_and_hides_them_in_debug() {
        let version = CalendarEventVersion::try_from("version-1".to_owned())
            .expect("synthetic version is valid");

        assert_eq!(version.as_str(), "version-1");
        assert!(!format!("{version:?}").contains("version-1"));
        assert_eq!(
            CalendarEventVersion::try_from(String::new()),
            Err(InvalidCalendarEventVersion)
        );
        assert_eq!(
            CalendarEventVersion::try_from("v".repeat(4097)),
            Err(InvalidCalendarEventVersion)
        );
        assert_eq!(
            CalendarEventVersion::try_from("version\n1".to_owned()),
            Err(InvalidCalendarEventVersion)
        );
    }

    #[test]
    fn update_draft_rejects_control_characters_in_the_subject() {
        assert!(
            CalendarEventUpdateDraft::timed(
                "Planning\tmeeting".to_owned(),
                timestamp(9),
                timestamp(10),
            )
            .is_err()
        );
    }

    #[test]
    fn update_requires_a_changed_subject_or_timed_range() {
        let event = occurrence("Planning", range());
        let editable = EditableCalendarEvent::new(
            event,
            CalendarEventVersion::try_from("version-1".to_owned())
                .expect("synthetic version is valid"),
        )
        .expect("synthetic timed event is editable");
        let draft =
            CalendarEventUpdateDraft::timed("Planning".to_owned(), timestamp(9), timestamp(10))
                .expect("synthetic update draft is valid");

        assert_eq!(
            CalendarEventUpdate::new(editable, draft),
            Err(NoCalendarEventChanges)
        );
    }

    #[test]
    fn confirms_only_the_same_account_view_event_and_requested_values() {
        let view = range();
        let source = EditableCalendarEvent::new(
            occurrence("Planning", view.clone()),
            CalendarEventVersion::try_from("version-1".to_owned())
                .expect("synthetic version is valid"),
        )
        .expect("synthetic timed event is editable");
        let draft = CalendarEventUpdateDraft::timed(
            "Updated planning".to_owned(),
            timestamp(11),
            timestamp(12),
        )
        .expect("synthetic update draft is valid");
        let update = CalendarEventUpdate::new(source, draft).expect("values changed");

        assert!(!update.confirms(&occurrence("Updated planning", view.clone())));
        let updated = updated_occurrence(view);
        assert!(update.confirms(&updated));
    }

    fn occurrence(subject: &str, range: CalendarViewReference) -> crate::CalendarOccurrence {
        let mut patch = CalendarOccurrencePatch::new();
        patch.set_subject(Some(subject.to_owned()));
        let mut time = crate::CalendarOccurrenceTimePatch::new();
        time.set_start(timestamp(9));
        time.set_end(timestamp(10));
        time.set_is_all_day(false);
        patch.set_time(time);
        patch
            .into_new(
                range,
                EventId::try_from("synthetic-event".to_owned())
                    .expect("synthetic event ID is valid"),
            )
            .expect("synthetic event is complete")
    }

    fn updated_occurrence(view: CalendarViewReference) -> crate::CalendarOccurrence {
        let mut patch = CalendarOccurrencePatch::new();
        patch.set_subject(Some("Updated planning".to_owned()));
        let mut time = crate::CalendarOccurrenceTimePatch::new();
        time.set_start(timestamp(11));
        time.set_end(timestamp(12));
        time.set_is_all_day(false);
        patch.set_time(time);
        patch
            .into_new(
                view,
                EventId::try_from("synthetic-event".to_owned())
                    .expect("synthetic event ID is valid"),
            )
            .expect("synthetic event is complete")
    }

    fn range() -> CalendarViewReference {
        let start_date = Date::from_calendar_date(2030, Month::January, 7)
            .expect("synthetic start date is valid");
        let end_date = Date::from_calendar_date(2030, Month::January, 14)
            .expect("synthetic end date is valid");
        let range = CalendarViewRange::new(
            start_date.midnight().assume_utc(),
            end_date.midnight().assume_utc(),
            start_date,
            end_date,
        )
        .expect("synthetic week is valid");
        CalendarViewReference::new(
            AccountId::try_from("synthetic-account".to_owned())
                .expect("synthetic account ID is valid"),
            range,
        )
    }

    fn timestamp(hour: u8) -> OffsetDateTime {
        Date::from_calendar_date(2030, Month::January, 8)
            .expect("synthetic date is valid")
            .with_hms(hour, 0, 0)
            .expect("synthetic time is valid")
            .assume_utc()
    }
}
