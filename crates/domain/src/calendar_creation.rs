//! Provider-independent values for creating a single timed calendar event.

use std::fmt;

use time::{OffsetDateTime, PrimitiveDateTime};
use uuid::Uuid;

use crate::{CalendarOccurrenceTime, CalendarViewReference};

/// A stable identifier for one calendar event creation attempt.
///
/// The value is sent as Microsoft Graph's `transactionId` and remains the same
/// when the application retries an unresolved creation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CalendarEventCreationId(Uuid);

impl CalendarEventCreationId {
    /// Creates a random identifier for a new user-requested operation.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Parses a persisted transaction identifier.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidCalendarEventCreationId`] when the value is not a UUID.
    pub fn parse(value: &str) -> Result<Self, InvalidCalendarEventCreationId> {
        Uuid::parse_str(value)
            .map(Self)
            .map_err(|_| InvalidCalendarEventCreationId)
    }

    /// Returns the UUID value used in the Graph request and encrypted cache.
    #[must_use]
    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for CalendarEventCreationId {
    fn default() -> Self {
        Self::new()
    }
}

/// A persisted creation identifier could not be parsed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidCalendarEventCreationId;

impl fmt::Display for InvalidCalendarEventCreationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("calendar creation identifier must be a UUID")
    }
}

impl std::error::Error for InvalidCalendarEventCreationId {}

/// A source time-zone identifier for the local schedule the user entered.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct CalendarTimeZoneId(Box<str>);

impl fmt::Debug for CalendarTimeZoneId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl CalendarTimeZoneId {
    /// Returns the time-zone identifier exactly as captured for the draft.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for CalendarTimeZoneId {
    type Error = InvalidCalendarEventTimeZone;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty()
            || value.len() > 255
            || value
                .chars()
                .any(|character| character.is_control() || character.is_whitespace())
        {
            return Err(InvalidCalendarEventTimeZone);
        }
        Ok(Self(value.into_boxed_str()))
    }
}

/// A local schedule used an invalid or empty time-zone identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidCalendarEventTimeZone;

impl fmt::Display for InvalidCalendarEventTimeZone {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("calendar event time zone must be a non-empty identifier")
    }
}

impl std::error::Error for InvalidCalendarEventTimeZone {}

/// A single timed event's source local schedule and resolved UTC range.
///
/// The source values preserve what the user entered. The UTC range is the
/// unambiguous instant range sent to Graph and used by the calendar cache.
#[derive(Clone, Eq, PartialEq)]
pub struct CalendarEventDraft {
    subject: Box<str>,
    local_start: PrimitiveDateTime,
    local_end: PrimitiveDateTime,
    time_zone: CalendarTimeZoneId,
    time: CalendarOccurrenceTime,
}

impl fmt::Debug for CalendarEventDraft {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CalendarEventDraft(<redacted>)")
    }
}

impl CalendarEventDraft {
    /// Creates a valid one-off timed event draft.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidCalendarEventDraft`] for a blank or multiline subject,
    /// unordered local values, or a UTC range that is empty, reversed, or not
    /// expressed with the UTC offset.
    pub fn timed(
        subject: String,
        local_start: PrimitiveDateTime,
        local_end: PrimitiveDateTime,
        time_zone: CalendarTimeZoneId,
        start: OffsetDateTime,
        end: OffsetDateTime,
    ) -> Result<Self, InvalidCalendarEventDraft> {
        let subject = subject.trim();
        if subject.is_empty() || subject.chars().any(char::is_control) || local_start >= local_end {
            return Err(InvalidCalendarEventDraft);
        }
        let time =
            CalendarOccurrenceTime::timed(start, end).map_err(|_| InvalidCalendarEventDraft)?;
        Ok(Self {
            subject: subject.into(),
            local_start,
            local_end,
            time_zone,
            time,
        })
    }

    /// Returns the user-entered subject.
    #[must_use]
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// Returns the user-entered local start date and time.
    #[must_use]
    pub fn local_start(&self) -> PrimitiveDateTime {
        self.local_start
    }

    /// Returns the user-entered local end date and time.
    #[must_use]
    pub fn local_end(&self) -> PrimitiveDateTime {
        self.local_end
    }

    /// Returns the source zone used to resolve the local schedule.
    #[must_use]
    pub fn time_zone(&self) -> &CalendarTimeZoneId {
        &self.time_zone
    }

    /// Returns the resolved UTC timed range.
    #[must_use]
    pub fn time(&self) -> &CalendarOccurrenceTime {
        &self.time
    }
}

/// A one-off event creation bound to an account's exact calendar view.
#[derive(Clone, Eq, PartialEq)]
pub struct CalendarEventCreation {
    id: CalendarEventCreationId,
    view: CalendarViewReference,
    draft: CalendarEventDraft,
}

impl fmt::Debug for CalendarEventCreation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CalendarEventCreation(<redacted>)")
    }
}

impl CalendarEventCreation {
    /// Creates a new operation with its own retry-stable identifier.
    #[must_use]
    pub fn new(view: CalendarViewReference, draft: CalendarEventDraft) -> Self {
        Self {
            id: CalendarEventCreationId::new(),
            view,
            draft,
        }
    }

    /// Restores an operation already validated by its encrypted persistence boundary.
    #[must_use]
    pub fn restore(
        id: CalendarEventCreationId,
        view: CalendarViewReference,
        draft: CalendarEventDraft,
    ) -> Self {
        Self { id, view, draft }
    }

    /// Returns the stable identifier reused for every retry of this operation.
    #[must_use]
    pub fn id(&self) -> CalendarEventCreationId {
        self.id
    }

    /// Returns the exact account and week this operation belongs to.
    #[must_use]
    pub fn view(&self) -> &CalendarViewReference {
        &self.view
    }

    /// Returns the immutable user-entered event draft.
    #[must_use]
    pub fn draft(&self) -> &CalendarEventDraft {
        &self.draft
    }
}

/// A calendar event draft contains invalid title or local-time values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidCalendarEventDraft;

impl fmt::Display for InvalidCalendarEventDraft {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("calendar event draft must contain a title and ordered timed values")
    }
}

impl std::error::Error for InvalidCalendarEventDraft {}

#[cfg(test)]
mod tests {
    use time::{Date, Month, PrimitiveDateTime, Time};

    use super::{CalendarEventCreation, CalendarEventDraft, CalendarTimeZoneId};
    use crate::{CalendarViewRange, CalendarViewReference};

    #[test]
    fn event_draft_keeps_source_local_values_and_resolved_utc_range_separate() {
        let view = view();
        let local_start = local(2030, Month::January, 7, 9, 30);
        let local_end = local(2030, Month::January, 7, 10, 0);
        let draft = CalendarEventDraft::timed(
            "  Planning  ".to_owned(),
            local_start,
            local_end,
            time_zone("Europe/Berlin"),
            local_start.assume_utc(),
            local_end.assume_utc(),
        )
        .expect("synthetic event draft is valid");
        let creation = CalendarEventCreation::new(view.clone(), draft.clone());
        let restored = CalendarEventCreation::restore(creation.id(), view, draft);

        assert_eq!(restored, creation);
        assert_eq!(restored.draft().subject(), "Planning");
        assert_eq!(restored.draft().local_start(), local_start);
        assert_eq!(restored.draft().time_zone().as_str(), "Europe/Berlin");
    }

    #[test]
    fn rejects_blank_subject_and_reversed_local_values() {
        let time_zone = time_zone("UTC");
        let start = local(2030, Month::January, 7, 10, 0);
        let end = local(2030, Month::January, 7, 9, 30);

        assert!(
            CalendarEventDraft::timed(
                "   ".to_owned(),
                start,
                end,
                time_zone.clone(),
                start.assume_utc(),
                end.assume_utc(),
            )
            .is_err()
        );
        assert!(
            CalendarEventDraft::timed(
                "Planning".to_owned(),
                start,
                end,
                time_zone,
                start.assume_utc(),
                end.assume_utc(),
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_empty_time_zone_identifiers() {
        assert!(CalendarTimeZoneId::try_from(String::new()).is_err());
        assert!(CalendarTimeZoneId::try_from("Europe/Bad Zone".to_owned()).is_err());
    }

    fn local(year: i32, month: Month, day: u8, hour: u8, minute: u8) -> PrimitiveDateTime {
        Date::from_calendar_date(year, month, day)
            .expect("synthetic date is valid")
            .with_time(Time::from_hms(hour, minute, 0).expect("synthetic time is valid"))
    }

    fn time_zone(value: &str) -> CalendarTimeZoneId {
        CalendarTimeZoneId::try_from(value.to_owned()).expect("synthetic time zone is valid")
    }

    fn view() -> CalendarViewReference {
        let start = Date::from_calendar_date(2030, Month::January, 7)
            .expect("synthetic date is valid")
            .midnight()
            .assume_utc();
        let end = Date::from_calendar_date(2030, Month::January, 14)
            .expect("synthetic date is valid")
            .midnight()
            .assume_utc();
        CalendarViewReference::new(
            "synthetic-account"
                .to_owned()
                .try_into()
                .expect("account id is valid"),
            CalendarViewRange::new(start, end, start.date(), end.date())
                .expect("synthetic view range is valid"),
        )
    }
}
