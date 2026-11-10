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
        let event = occurrence("Planning", primary_calendar_view());
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
    fn confirmation_rejects_a_different_account() {
        let update = event_update();
        let event = occurrence_with_confirmation_fields(
            view_for_account("synthetic-other-account"),
            "synthetic-event",
            "Updated planning",
            (timestamp(11), timestamp(12)),
        );

        assert!(!update.confirms(&event));
    }

    #[test]
    fn confirmation_rejects_a_different_view_range() {
        let update = event_update();
        let event = occurrence_with_confirmation_fields(
            view_with_shifted_range(),
            "synthetic-event",
            "Updated planning",
            (timestamp(11), timestamp(12)),
        );

        assert!(!update.confirms(&event));
    }

    #[test]
    fn confirmation_rejects_a_different_event_id() {
        let update = event_update();
        let event = occurrence_with_confirmation_fields(
            primary_calendar_view(),
            "synthetic-other-event",
            "Updated planning",
            (timestamp(11), timestamp(12)),
        );

        assert!(!update.confirms(&event));
    }

    #[test]
    fn confirmation_rejects_a_different_subject() {
        let update = event_update();
        let event = occurrence_with_confirmation_fields(
            primary_calendar_view(),
            "synthetic-event",
            "Different planning",
            (timestamp(11), timestamp(12)),
        );

        assert!(!update.confirms(&event));
    }

    #[test]
    fn confirmation_rejects_a_different_time_range() {
        let update = event_update();
        let event = occurrence_with_confirmation_fields(
            primary_calendar_view(),
            "synthetic-event",
            "Updated planning",
            (timestamp(9), timestamp(10)),
        );

        assert!(!update.confirms(&event));
    }

    #[test]
    fn confirmation_accepts_the_same_event_and_requested_values() {
        let update = event_update();
        let event = occurrence_with_confirmation_fields(
            primary_calendar_view(),
            "synthetic-event",
            "Updated planning",
            (timestamp(11), timestamp(12)),
        );

        assert!(update.confirms(&event));
    }

    fn event_update() -> CalendarEventUpdate {
        let view = primary_calendar_view();
        let source = EditableCalendarEvent::new(
            occurrence("Planning", view),
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
        CalendarEventUpdate::new(source, draft).expect("values changed")
    }

    fn occurrence(subject: &str, range: CalendarViewReference) -> crate::CalendarOccurrence {
        occurrence_with_confirmation_fields(
            range,
            "synthetic-event",
            subject,
            (timestamp(9), timestamp(10)),
        )
    }

    fn occurrence_with_confirmation_fields(
        reference: CalendarViewReference,
        id: &str,
        subject: &str,
        time_range: (OffsetDateTime, OffsetDateTime),
    ) -> crate::CalendarOccurrence {
        let mut patch = CalendarOccurrencePatch::new();
        patch.set_subject(Some(subject.to_owned()));
        let (start, end) = time_range;
        let mut time = crate::CalendarOccurrenceTimePatch::new();
        time.set_start(start);
        time.set_end(end);
        time.set_is_all_day(false);
        patch.set_time(time);
        patch
            .into_new(
                reference,
                EventId::try_from(id.to_owned()).expect("synthetic event ID is valid"),
            )
            .expect("synthetic event is complete")
    }

    fn primary_calendar_view() -> CalendarViewReference {
        view_for_account("synthetic-account")
    }

    fn view_for_account(account_id: &str) -> CalendarViewReference {
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
            AccountId::try_from(account_id.to_owned()).expect("synthetic account ID is valid"),
            range,
        )
    }

    fn view_with_shifted_range() -> CalendarViewReference {
        let view = primary_calendar_view();
        let source_range = view.range();
        let range = CalendarViewRange::new(
            source_range.start() + time::Duration::hours(1),
            source_range.end(),
            source_range.start_date(),
            source_range.end_date_exclusive(),
        )
        .expect("synthetic shifted week is valid");
        CalendarViewReference::new(view.account_id().clone(), range)
    }

    fn timestamp(hour: u8) -> OffsetDateTime {
        Date::from_calendar_date(2030, Month::January, 8)
            .expect("synthetic date is valid")
            .with_hms(hour, 0, 0)
            .expect("synthetic time is valid")
            .assume_utc()
    }
}
