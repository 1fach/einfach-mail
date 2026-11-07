//! Provider-independent values for creating a single timed calendar event.

use std::fmt;

use time::{OffsetDateTime, PrimitiveDateTime};
use uuid::Uuid;

use crate::{
    CalendarOccurrenceTime, CalendarViewReference, calendar_subject::normalized_event_subject,
};

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
        formatter.write_str("CalendarTimeZoneId(<redacted>)")
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
    /// Returns [`InvalidCalendarEventDraft`] for a blank or control-containing subject,
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
        let subject = normalized_event_subject(&subject).ok_or(InvalidCalendarEventDraft)?;
        if local_start >= local_end {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CalendarEventCreationScope {
    Bounded,
    LegacyUnbounded,
}

/// A one-off event creation scoped to an account's calendar view.
#[derive(Clone, Eq, PartialEq)]
pub struct CalendarEventCreation {
    id: CalendarEventCreationId,
    view: CalendarViewReference,
    draft: CalendarEventDraft,
    scope: CalendarEventCreationScope,
}

impl fmt::Debug for CalendarEventCreation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CalendarEventCreation(<redacted>)")
    }
}

impl CalendarEventCreation {
    /// Creates an operation whose endpoints fit the selected calendar view.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidCalendarEventCreation`] when either local endpoint date
    /// or resolved UTC endpoint falls outside the view.
    pub fn try_new(
        view: CalendarViewReference,
        draft: CalendarEventDraft,
    ) -> Result<Self, InvalidCalendarEventCreation> {
        Self::try_from_parts(CalendarEventCreationId::new(), view, draft)
    }

    /// Restores a pending operation after validating its endpoints against the view.
    ///
    /// Retains `id` so the operation keeps its retry identifier.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidCalendarEventCreation`] when either local endpoint date
    /// or resolved UTC endpoint falls outside the view.
    pub fn try_restore(
        id: CalendarEventCreationId,
        view: CalendarViewReference,
        draft: CalendarEventDraft,
    ) -> Result<Self, InvalidCalendarEventCreation> {
        Self::try_from_parts(id, view, draft)
    }

    /// Restores a pending operation written before creations were bounded to the selected view.
    ///
    /// Persistence uses this only for markers migrated from the older format.
    /// New operations must use [`Self::try_new`] or [`Self::try_restore`].
    #[must_use]
    pub fn restore_legacy_pending(
        id: CalendarEventCreationId,
        view: CalendarViewReference,
        draft: CalendarEventDraft,
    ) -> Self {
        Self {
            id,
            view,
            draft,
            scope: CalendarEventCreationScope::LegacyUnbounded,
        }
    }

    fn try_from_parts(
        id: CalendarEventCreationId,
        view: CalendarViewReference,
        draft: CalendarEventDraft,
    ) -> Result<Self, InvalidCalendarEventCreation> {
        if !draft_fits_view(&draft, view.range()) {
            return Err(InvalidCalendarEventCreation);
        }
        Ok(Self {
            id,
            view,
            draft,
            scope: CalendarEventCreationScope::Bounded,
        })
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

    /// Returns whether this operation came from the earlier unbounded format.
    #[must_use]
    pub fn is_legacy_pending(&self) -> bool {
        self.scope == CalendarEventCreationScope::LegacyUnbounded
    }
}

/// An event creation retained because its result or local cleanup still needs resolution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PendingCalendarEventCreation {
    /// The request may have reached Graph, or its accepted event is not yet cached.
    Unresolved(CalendarEventCreation),
    /// Graph rejected the request and only local marker cleanup remains.
    Rejected(CalendarEventCreation),
}

impl PendingCalendarEventCreation {
    /// Returns the operation retained for resolution.
    #[must_use]
    pub fn creation(&self) -> &CalendarEventCreation {
        match self {
            Self::Unresolved(creation) | Self::Rejected(creation) => creation,
        }
    }
}

fn draft_fits_view(draft: &CalendarEventDraft, range: &crate::CalendarViewRange) -> bool {
    let start_date = draft.local_start().date();
    let end_date = draft.local_end().date();
    let local_dates_fit = range.start_date() <= start_date
        && start_date < range.end_date_exclusive()
        && range.start_date() <= end_date
        && end_date < range.end_date_exclusive();
    let CalendarOccurrenceTime::Timed { start, end } = draft.time() else {
        return false;
    };
    local_dates_fit && *start >= range.start() && *end < range.end()
}

/// A calendar event's local or resolved endpoints fall outside its selected view.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidCalendarEventCreation;

impl fmt::Display for InvalidCalendarEventCreation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("calendar event endpoints must fit the selected view")
    }
}

impl std::error::Error for InvalidCalendarEventCreation {}

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

    use super::{
        CalendarEventCreation, CalendarEventCreationId, CalendarEventDraft, CalendarTimeZoneId,
    };
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
        let creation = CalendarEventCreation::try_new(view.clone(), draft.clone())
            .expect("synthetic creation fits its week");
        let restored = CalendarEventCreation::try_restore(creation.id(), view, draft)
            .expect("synthetic restored event fits its week");

        assert_eq!(restored, creation);
        assert!(!restored.is_legacy_pending());
        assert_eq!(restored.draft().subject(), "Planning");
        assert_eq!(restored.draft().local_start(), local_start);
        assert_eq!(restored.draft().time_zone().as_str(), "Europe/Berlin");
    }

    #[test]
    fn time_zone_identifier_debug_output_is_redacted() {
        let zone = time_zone("Pacific/Chatham");

        let debug = format!("{zone:?}");

        assert_eq!(zone.as_str(), "Pacific/Chatham");
        assert!(
            debug.contains("<redacted>"),
            "missing redaction marker: {debug}"
        );
        assert!(
            !debug.contains("Pacific/Chatham"),
            "Debug output included a time-zone identifier: {debug}"
        );
    }

    #[test]
    fn rejects_creation_endpoints_outside_local_dates_or_utc_view_instants() {
        let time_zone = time_zone("UTC");
        let view = view_for_system_offset(-3);
        let local_start = local(2030, Month::January, 13, 23, 30);
        let local_end = local(2030, Month::January, 14, 0, 30);
        let draft = CalendarEventDraft::timed(
            "Planning".to_owned(),
            local_start,
            local_end,
            time_zone.clone(),
            local_start.assume_utc(),
            local_end.assume_utc(),
        )
        .expect("synthetic event draft is valid");
        let id = CalendarEventCreationId::new();
        assert!(CalendarEventCreation::try_new(view.clone(), draft.clone()).is_err());
        assert!(CalendarEventCreation::try_restore(id, view, draft).is_err());

        let view = view_for_system_offset(3);
        let local_start = local(2030, Month::January, 13, 20, 30);
        let local_end = local(2030, Month::January, 13, 21, 30);
        let draft = CalendarEventDraft::timed(
            "Planning".to_owned(),
            local_start,
            local_end,
            time_zone,
            local_start.assume_utc(),
            local_end.assume_utc(),
        )
        .expect("synthetic event draft is valid");
        let id = CalendarEventCreationId::new();
        assert!(CalendarEventCreation::try_new(view.clone(), draft.clone()).is_err());
        assert!(CalendarEventCreation::try_restore(id, view, draft).is_err());
    }

    #[test]
    fn legacy_pending_creation_can_be_restored_outside_its_view() {
        let view = view();
        let local_start = local(2030, Month::January, 7, 0, 30);
        let local_end = local(2030, Month::January, 7, 1, 0);
        let draft = CalendarEventDraft::timed(
            "Planning".to_owned(),
            local_start,
            local_end,
            time_zone("Pacific/Kiritimati"),
            local(2030, Month::January, 6, 10, 30).assume_utc(),
            local(2030, Month::January, 6, 11, 0).assume_utc(),
        )
        .expect("synthetic legacy event draft is valid");
        let id = CalendarEventCreationId::new();

        assert!(CalendarEventCreation::try_restore(id, view.clone(), draft.clone()).is_err());
        let restored = CalendarEventCreation::restore_legacy_pending(id, view, draft);

        assert_eq!(restored.id(), id);
        assert!(restored.is_legacy_pending());
        assert_eq!(restored.draft().local_start(), local_start);
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

    fn view_for_system_offset(offset_hours: i64) -> CalendarViewReference {
        let start_date =
            Date::from_calendar_date(2030, Month::January, 7).expect("synthetic date is valid");
        let end_date =
            Date::from_calendar_date(2030, Month::January, 14).expect("synthetic date is valid");
        let start = start_date.midnight().assume_utc() - time::Duration::hours(offset_hours);
        let end = end_date.midnight().assume_utc() - time::Duration::hours(offset_hours);
        CalendarViewReference::new(
            "synthetic-account"
                .to_owned()
                .try_into()
                .expect("account id is valid"),
            CalendarViewRange::new(start, end, start_date, end_date)
                .expect("synthetic fixed-offset view range is valid"),
        )
    }
}
