//! Provider-independent calendar occurrence and view concepts.

use std::fmt;

use time::{Date, OffsetDateTime, Time, UtcOffset};

use crate::{AccountId, EventId};

/// An invalid or empty calendar view range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidCalendarViewRange;

impl fmt::Display for InvalidCalendarViewRange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("calendar view bounds must be non-empty and ordered")
    }
}

impl std::error::Error for InvalidCalendarViewRange {}

/// A fixed calendar view's UTC instants and local date bounds.
///
/// The instant bounds are sent to the provider and scope its delta cursor. The
/// date bounds preserve the user's local week for all-day event filtering.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct CalendarViewRange {
    start: OffsetDateTime,
    end: OffsetDateTime,
    start_date: Date,
    end_date_exclusive: Date,
}

impl fmt::Debug for CalendarViewRange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CalendarViewRange(<redacted>)")
    }
}

impl CalendarViewRange {
    /// Creates an ordered, non-empty view range.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidCalendarViewRange`] when either range is empty or reversed,
    /// or when either instant does not use the UTC offset.
    pub fn new(
        start: OffsetDateTime,
        end: OffsetDateTime,
        start_date: Date,
        end_date_exclusive: Date,
    ) -> Result<Self, InvalidCalendarViewRange> {
        if start >= end
            || start.offset() != UtcOffset::UTC
            || end.offset() != UtcOffset::UTC
            || start_date >= end_date_exclusive
        {
            return Err(InvalidCalendarViewRange);
        }
        Ok(Self {
            start,
            end,
            start_date,
            end_date_exclusive,
        })
    }

    /// Returns the inclusive UTC start instant sent to the provider.
    #[must_use]
    pub fn start(&self) -> OffsetDateTime {
        self.start
    }

    /// Returns the exclusive UTC end instant sent to the provider.
    #[must_use]
    pub fn end(&self) -> OffsetDateTime {
        self.end
    }

    /// Returns the first local calendar date included in the view.
    #[must_use]
    pub fn start_date(&self) -> Date {
        self.start_date
    }

    /// Returns the first local calendar date excluded from the view.
    #[must_use]
    pub fn end_date_exclusive(&self) -> Date {
        self.end_date_exclusive
    }
}

/// An account-scoped view of the user's primary calendar.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct CalendarViewReference {
    account_id: AccountId,
    range: CalendarViewRange,
}

impl fmt::Debug for CalendarViewReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CalendarViewReference(<redacted>)")
    }
}

impl CalendarViewReference {
    /// Creates a view reference for one account and one fixed date range.
    #[must_use]
    pub fn new(account_id: AccountId, range: CalendarViewRange) -> Self {
        Self { account_id, range }
    }

    /// Returns the account that scopes this view.
    #[must_use]
    pub fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns the fixed instant and local date bounds for this view.
    #[must_use]
    pub fn range(&self) -> &CalendarViewRange {
        &self.range
    }
}

/// An invalid timed or all-day occurrence range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidCalendarOccurrenceTime;

impl fmt::Display for InvalidCalendarOccurrenceTime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("calendar occurrence bounds must be non-empty and ordered")
    }
}

impl std::error::Error for InvalidCalendarOccurrenceTime {}

/// The time values for one Graph-expanded calendar occurrence.
#[derive(Clone, Eq, PartialEq)]
pub enum CalendarOccurrenceTime {
    /// A half-open range of UTC instants.
    Timed {
        /// The inclusive UTC start instant.
        start: OffsetDateTime,
        /// The exclusive UTC end instant.
        end: OffsetDateTime,
    },
    /// A half-open range of calendar dates that must not be timezone-converted.
    AllDay {
        /// The first date occupied by the event.
        start: Date,
        /// The first date not occupied by the event.
        end_exclusive: Date,
    },
}

impl fmt::Debug for CalendarOccurrenceTime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Timed { .. } => "Timed(<redacted>)",
            Self::AllDay { .. } => "AllDay(<redacted>)",
        })
    }
}

impl CalendarOccurrenceTime {
    /// Creates a non-empty timed occurrence range.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidCalendarOccurrenceTime`] when `start` is not earlier than
    /// `end` or either instant does not use the UTC offset.
    pub fn timed(
        start: OffsetDateTime,
        end: OffsetDateTime,
    ) -> Result<Self, InvalidCalendarOccurrenceTime> {
        if start >= end || start.offset() != UtcOffset::UTC || end.offset() != UtcOffset::UTC {
            return Err(InvalidCalendarOccurrenceTime);
        }
        Ok(Self::Timed { start, end })
    }

    /// Creates a non-empty all-day date range.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidCalendarOccurrenceTime`] when `start` is not earlier than `end_exclusive`.
    pub fn all_day(
        start: Date,
        end_exclusive: Date,
    ) -> Result<Self, InvalidCalendarOccurrenceTime> {
        if start >= end_exclusive {
            return Err(InvalidCalendarOccurrenceTime);
        }
        Ok(Self::AllDay {
            start,
            end_exclusive,
        })
    }

    /// Returns whether the event overlaps the exact instant and date bounds of `range`.
    #[must_use]
    pub fn overlaps(&self, range: &CalendarViewRange) -> bool {
        match self {
            Self::Timed { start, end } => *start < range.end && *end > range.start,
            Self::AllDay {
                start,
                end_exclusive,
            } => *start < range.end_date_exclusive && *end_exclusive > range.start_date,
        }
    }
}

/// A field value in a sparse provider update.
#[derive(Clone, Default, Eq, PartialEq)]
pub enum CalendarField<T> {
    /// The provider omitted the field, so a cached value stays unchanged.
    #[default]
    Unchanged,
    /// The provider supplied a new value.
    Set(T),
    /// The provider explicitly cleared an optional field.
    Clear,
}

impl<T> fmt::Debug for CalendarField<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unchanged => "Unchanged",
            Self::Set(_) => "Set(<redacted>)",
            Self::Clear => "Clear",
        })
    }
}

/// Sparse updates to one calendar occurrence.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct CalendarOccurrencePatch {
    subject: CalendarField<String>,
    location: CalendarField<String>,
    time: Option<CalendarOccurrenceTimePatch>,
    original_start_time_zone: CalendarField<String>,
    original_end_time_zone: CalendarField<String>,
}

impl CalendarOccurrencePatch {
    /// Creates a patch with no changed fields.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets or clears the occurrence subject.
    pub fn set_subject(&mut self, subject: Option<String>) {
        self.subject = field_from_option(subject);
    }

    /// Sets or clears the occurrence location.
    pub fn set_location(&mut self, location: Option<String>) {
        self.location = field_from_option(location);
    }

    /// Replaces the occurrence's changed time fields.
    pub fn set_time(&mut self, time: CalendarOccurrenceTimePatch) {
        self.time = Some(time);
    }

    /// Sets or clears the original start time-zone identifier.
    pub fn set_original_start_time_zone(&mut self, time_zone: Option<String>) {
        self.original_start_time_zone = field_from_option(time_zone);
    }

    /// Sets or clears the original end time-zone identifier.
    pub fn set_original_end_time_zone(&mut self, time_zone: Option<String>) {
        self.original_end_time_zone = field_from_option(time_zone);
    }

    /// Applies these changed fields to a cached occurrence.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidCalendarOccurrenceTime`] when the changed time fields do
    /// not form a non-empty UTC range. All-day bounds must be UTC midnight.
    pub fn apply_to(
        self,
        occurrence: &mut CalendarOccurrence,
    ) -> Result<(), InvalidCalendarOccurrenceTime> {
        let updated_time = self
            .time
            .map(|time| time.apply_to(&occurrence.time))
            .transpose()?;
        apply_optional_field(self.subject, &mut occurrence.subject);
        apply_optional_field(self.location, &mut occurrence.location);
        if let Some(time) = updated_time {
            occurrence.time = time;
        }
        apply_optional_field(
            self.original_start_time_zone,
            &mut occurrence.original_start_time_zone,
        );
        apply_optional_field(
            self.original_end_time_zone,
            &mut occurrence.original_end_time_zone,
        );
        Ok(())
    }

    /// Creates an occurrence from a patch returned for a new event.
    ///
    /// # Errors
    ///
    /// Returns [`NewCalendarOccurrenceError::Incomplete`] when the patch omits
    /// the start, end, or all-day flag. Returns
    /// [`NewCalendarOccurrenceError::InvalidTime`] when those fields do not form
    /// a non-empty UTC range. All-day bounds must be UTC midnight.
    pub fn into_new(
        self,
        reference: CalendarViewReference,
        id: EventId,
    ) -> Result<CalendarOccurrence, NewCalendarOccurrenceError> {
        let time_patch = self.time.ok_or(NewCalendarOccurrenceError::Incomplete)?;
        let time = time_patch.into_complete()?;
        Ok(CalendarOccurrence {
            reference,
            id,
            subject: field_value(self.subject),
            location: field_value(self.location),
            time,
            original_start_time_zone: field_value(self.original_start_time_zone),
            original_end_time_zone: field_value(self.original_end_time_zone),
        })
    }
}

/// Changed fields in a provider's occurrence time update.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct CalendarOccurrenceTimePatch {
    start: Option<OffsetDateTime>,
    end: Option<OffsetDateTime>,
    is_all_day: Option<bool>,
}

impl fmt::Debug for CalendarOccurrenceTimePatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CalendarOccurrenceTimePatch")
            .field("start", &self.start.as_ref().map(|_| "<redacted>"))
            .field("end", &self.end.as_ref().map(|_| "<redacted>"))
            .field("is_all_day", &self.is_all_day)
            .finish()
    }
}

impl CalendarOccurrenceTimePatch {
    /// Creates a time patch with no changed fields.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets a changed UTC start instant.
    pub fn set_start(&mut self, start: OffsetDateTime) {
        self.start = Some(start);
    }

    /// Sets a changed UTC end instant.
    pub fn set_end(&mut self, end: OffsetDateTime) {
        self.end = Some(end);
    }

    /// Sets whether the changed range represents all-day dates.
    pub fn set_is_all_day(&mut self, is_all_day: bool) {
        self.is_all_day = Some(is_all_day);
    }

    fn into_complete(self) -> Result<CalendarOccurrenceTime, NewCalendarOccurrenceError> {
        let start = self.start.ok_or(NewCalendarOccurrenceError::Incomplete)?;
        let end = self.end.ok_or(NewCalendarOccurrenceError::Incomplete)?;
        let is_all_day = self
            .is_all_day
            .ok_or(NewCalendarOccurrenceError::Incomplete)?;
        build_occurrence_time(start, end, is_all_day)
            .map_err(NewCalendarOccurrenceError::InvalidTime)
    }

    fn apply_to(
        self,
        current: &CalendarOccurrenceTime,
    ) -> Result<CalendarOccurrenceTime, InvalidCalendarOccurrenceTime> {
        let (current_start, current_end, current_all_day) = match current {
            CalendarOccurrenceTime::Timed { start, end } => (*start, *end, false),
            CalendarOccurrenceTime::AllDay {
                start,
                end_exclusive,
            } => (
                start.with_time(Time::MIDNIGHT).assume_utc(),
                end_exclusive.with_time(Time::MIDNIGHT).assume_utc(),
                true,
            ),
        };
        let start = self.start.unwrap_or(current_start);
        let end = self.end.unwrap_or(current_end);
        let is_all_day = self.is_all_day.unwrap_or(current_all_day);
        build_occurrence_time(start, end, is_all_day)
    }
}

fn build_occurrence_time(
    start: OffsetDateTime,
    end: OffsetDateTime,
    is_all_day: bool,
) -> Result<CalendarOccurrenceTime, InvalidCalendarOccurrenceTime> {
    if start.offset() != UtcOffset::UTC || end.offset() != UtcOffset::UTC {
        return Err(InvalidCalendarOccurrenceTime);
    }
    if is_all_day {
        if start.time() != Time::MIDNIGHT || end.time() != Time::MIDNIGHT {
            return Err(InvalidCalendarOccurrenceTime);
        }
        CalendarOccurrenceTime::all_day(start.date(), end.date())
    } else {
        CalendarOccurrenceTime::timed(start, end)
    }
}

impl fmt::Debug for CalendarOccurrencePatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CalendarOccurrencePatch")
            .field("subject", &redacted_field(&self.subject))
            .field("location", &redacted_field(&self.location))
            .field("time", &self.time.as_ref().map(|_| "<redacted>"))
            .field(
                "original_start_time_zone",
                &redacted_field(&self.original_start_time_zone),
            )
            .field(
                "original_end_time_zone",
                &redacted_field(&self.original_end_time_zone),
            )
            .finish()
    }
}

/// A new calendar occurrence lacks required time fields or has invalid time bounds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NewCalendarOccurrenceError {
    /// The provider omitted the start, end, or all-day flag.
    Incomplete,
    /// The complete time fields do not form a valid UTC range.
    InvalidTime(InvalidCalendarOccurrenceTime),
}

impl fmt::Display for NewCalendarOccurrenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Incomplete => {
                formatter.write_str("a new calendar occurrence is missing required time fields")
            }
            Self::InvalidTime(error) => fmt::Display::fmt(error, formatter),
        }
    }
}

impl std::error::Error for NewCalendarOccurrenceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Incomplete => None,
            Self::InvalidTime(error) => Some(error),
        }
    }
}

/// One provider-independent occurrence in an account's primary calendar view.
#[derive(Clone, Eq, PartialEq)]
pub struct CalendarOccurrence {
    reference: CalendarViewReference,
    id: EventId,
    subject: Option<String>,
    location: Option<String>,
    time: CalendarOccurrenceTime,
    original_start_time_zone: Option<String>,
    original_end_time_zone: Option<String>,
}

impl CalendarOccurrence {
    /// Returns the account and fixed view range that scope this occurrence.
    #[must_use]
    pub fn reference(&self) -> &CalendarViewReference {
        &self.reference
    }

    /// Returns the provider-issued event identifier.
    #[must_use]
    pub fn id(&self) -> &EventId {
        &self.id
    }

    /// Returns the optional subject supplied by the provider.
    #[must_use]
    pub fn subject(&self) -> Option<&str> {
        self.subject.as_deref()
    }

    /// Returns the optional location supplied by the provider.
    #[must_use]
    pub fn location(&self) -> Option<&str> {
        self.location.as_deref()
    }

    /// Returns this occurrence's timed instant range or all-day date range.
    #[must_use]
    pub fn time(&self) -> &CalendarOccurrenceTime {
        &self.time
    }

    /// Returns the original start time-zone identifier without mapping it.
    #[must_use]
    pub fn original_start_time_zone(&self) -> Option<&str> {
        self.original_start_time_zone.as_deref()
    }

    /// Returns the original end time-zone identifier without mapping it.
    #[must_use]
    pub fn original_end_time_zone(&self) -> Option<&str> {
        self.original_end_time_zone.as_deref()
    }

    /// Returns whether this occurrence intersects its referenced view range.
    #[must_use]
    pub fn overlaps_view(&self) -> bool {
        self.time.overlaps(self.reference.range())
    }
}

impl fmt::Debug for CalendarOccurrence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CalendarOccurrence")
            .field("reference", &"<redacted>")
            .field("id", &"<redacted>")
            .field("subject", &self.subject.as_ref().map(|_| "<redacted>"))
            .field("location", &self.location.as_ref().map(|_| "<redacted>"))
            .field("time", &"<redacted>")
            .field(
                "original_start_time_zone",
                &self.original_start_time_zone.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "original_end_time_zone",
                &self.original_end_time_zone.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// A calendar occurrence added, changed, or removed from a view.
#[derive(Clone, Eq, PartialEq)]
pub enum CalendarDeltaChange {
    /// Applies sparse fields to a known occurrence or creates a complete new one.
    Upsert {
        /// The provider-issued event identifier.
        id: EventId,
        /// The changed occurrence fields.
        patch: CalendarOccurrencePatch,
    },
    /// Removes an occurrence from the cached view when it is present.
    Removed {
        /// The provider-issued event identifier.
        id: EventId,
    },
}

impl fmt::Debug for CalendarDeltaChange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Upsert { patch, .. } => formatter
                .debug_struct("Upsert")
                .field("id", &"<redacted>")
                .field("patch", patch)
                .finish(),
            Self::Removed { .. } => formatter
                .debug_struct("Removed")
                .field("id", &"<redacted>")
                .finish(),
        }
    }
}

/// Opaque continuation URL returned by a calendar delta response.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct CalendarDeltaLink(Box<str>);

impl CalendarDeltaLink {
    /// Keeps the exact provider-issued URL for persistence or resumption.
    ///
    /// This constructor does not validate the URL. The provider must validate
    /// it against its authority before attaching an access token.
    #[must_use]
    pub fn from_opaque(link: String) -> Self {
        Self(link.into_boxed_str())
    }

    /// Returns the exact URL that must be sent back to its provider.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CalendarDeltaLink {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CalendarDeltaLink(<redacted>)")
    }
}

/// Whether a delta page has another page or finishes the current sync round.
#[derive(Clone, Eq, PartialEq)]
pub enum CalendarDeltaProgress {
    /// More changes remain in the current round.
    Next(CalendarDeltaLink),
    /// The round is complete and this link starts the next round.
    Complete(CalendarDeltaLink),
}

impl fmt::Debug for CalendarDeltaProgress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Next(_) => formatter.write_str("Next(<redacted>)"),
            Self::Complete(_) => formatter.write_str("Complete(<redacted>)"),
        }
    }
}

/// Changes and the continuation state from one calendar delta response page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CalendarDeltaPage {
    changes: Vec<CalendarDeltaChange>,
    progress: CalendarDeltaProgress,
}

impl CalendarDeltaPage {
    /// Creates a page from validated occurrence changes and cursor progress.
    #[must_use]
    pub fn new(changes: Vec<CalendarDeltaChange>, progress: CalendarDeltaProgress) -> Self {
        Self { changes, progress }
    }

    /// Returns the changes in this page.
    #[must_use]
    pub fn changes(&self) -> &[CalendarDeltaChange] {
        &self.changes
    }

    /// Returns the page's continuation state.
    #[must_use]
    pub fn progress(&self) -> &CalendarDeltaProgress {
        &self.progress
    }

    /// Returns the changes and continuation state.
    #[must_use]
    pub fn into_parts(self) -> (Vec<CalendarDeltaChange>, CalendarDeltaProgress) {
        (self.changes, self.progress)
    }
}

fn field_from_option<T>(field_value: Option<T>) -> CalendarField<T> {
    match field_value {
        Some(field_value) => CalendarField::Set(field_value),
        None => CalendarField::Clear,
    }
}

fn field_value<T>(field: CalendarField<T>) -> Option<T> {
    match field {
        CalendarField::Unchanged | CalendarField::Clear => None,
        CalendarField::Set(set_value) => Some(set_value),
    }
}

fn apply_optional_field<T>(field: CalendarField<T>, target: &mut Option<T>) {
    match field {
        CalendarField::Unchanged => {}
        CalendarField::Set(set_value) => *target = Some(set_value),
        CalendarField::Clear => *target = None,
    }
}

fn redacted_field<T>(field: &CalendarField<T>) -> &'static str {
    match field {
        CalendarField::Unchanged => "unchanged",
        CalendarField::Set(_) => "<redacted>",
        CalendarField::Clear => "cleared",
    }
}

#[cfg(test)]
mod tests {
    use time::{Date, Duration, Month, OffsetDateTime};

    use super::{
        CalendarDeltaChange, CalendarDeltaLink, CalendarDeltaProgress, CalendarField,
        CalendarOccurrence, CalendarOccurrencePatch, CalendarOccurrenceTime,
        CalendarOccurrenceTimePatch, CalendarViewRange, CalendarViewReference,
        InvalidCalendarOccurrenceTime, InvalidCalendarViewRange, NewCalendarOccurrenceError,
    };
    use crate::{AccountId, EventId};

    fn account_id() -> AccountId {
        AccountId::try_from("synthetic-account".to_owned())
            .expect("synthetic account ID is not empty")
    }

    fn event_id() -> EventId {
        EventId::try_from("synthetic-event".to_owned()).expect("synthetic event ID is not empty")
    }

    fn date(year: i32, month: u8, day: u8) -> Date {
        let month = Month::try_from(month).expect("synthetic month is valid");
        Date::from_calendar_date(year, month, day).expect("synthetic date is valid")
    }

    fn reference() -> CalendarViewReference {
        CalendarViewReference::new(
            account_id(),
            CalendarViewRange::new(
                OffsetDateTime::UNIX_EPOCH,
                OffsetDateTime::UNIX_EPOCH + Duration::days(1),
                date(2026, 10, 26),
                date(2026, 11, 2),
            )
            .expect("synthetic view range is ordered"),
        )
    }

    fn occurrence(patch: CalendarOccurrencePatch) -> CalendarOccurrence {
        patch
            .into_new(reference(), event_id())
            .expect("synthetic occurrence has a time range")
    }

    #[test]
    fn validates_non_empty_instant_and_date_ranges() {
        assert_eq!(
            CalendarViewRange::new(
                OffsetDateTime::UNIX_EPOCH,
                OffsetDateTime::UNIX_EPOCH,
                date(2026, 10, 26),
                date(2026, 11, 2)
            ),
            Err(InvalidCalendarViewRange)
        );
        assert_eq!(
            CalendarViewRange::new(
                OffsetDateTime::UNIX_EPOCH,
                OffsetDateTime::UNIX_EPOCH + Duration::seconds(10),
                date(2026, 11, 2),
                date(2026, 10, 26)
            ),
            Err(InvalidCalendarViewRange)
        );
        assert_eq!(
            CalendarOccurrenceTime::timed(
                OffsetDateTime::UNIX_EPOCH + Duration::seconds(10),
                OffsetDateTime::UNIX_EPOCH
            ),
            Err(InvalidCalendarOccurrenceTime)
        );
        assert_eq!(
            CalendarOccurrenceTime::all_day(date(2026, 11, 2), date(2026, 10, 26)),
            Err(InvalidCalendarOccurrenceTime)
        );
    }

    #[test]
    fn event_ranges_overlap_the_correct_local_week_bounds() {
        let range = reference().range().clone();
        let timed = CalendarOccurrenceTime::timed(
            OffsetDateTime::UNIX_EPOCH + Duration::seconds(86_300),
            OffsetDateTime::UNIX_EPOCH + Duration::seconds(86_500),
        )
        .expect("synthetic timed range is ordered");
        let all_day = CalendarOccurrenceTime::all_day(date(2026, 11, 1), date(2026, 11, 3))
            .expect("synthetic all-day range is ordered");
        let before_view = CalendarOccurrenceTime::all_day(date(2026, 10, 24), date(2026, 10, 26))
            .expect("synthetic all-day range is ordered");

        assert!(timed.overlaps(&range));
        assert!(all_day.overlaps(&range));
        assert!(!before_view.overlaps(&range));
    }

    #[test]
    fn sparse_patch_preserves_omitted_fields_and_clears_explicit_nulls() {
        let mut initial = CalendarOccurrencePatch::new();
        initial.set_subject(Some("Planning".to_owned()));
        initial.set_location(Some("Room A".to_owned()));
        let mut time_patch = CalendarOccurrenceTimePatch::new();
        time_patch.set_start(OffsetDateTime::UNIX_EPOCH);
        time_patch.set_end(OffsetDateTime::UNIX_EPOCH + Duration::hours(1));
        time_patch.set_is_all_day(false);
        initial.set_time(time_patch);
        initial.set_original_start_time_zone(Some("Custom/Source".to_owned()));
        let mut cached = occurrence(initial);

        let mut sparse = CalendarOccurrencePatch::new();
        sparse.set_location(None);
        sparse
            .apply_to(&mut cached)
            .expect("synthetic sparse patch preserves a valid occurrence");

        assert_eq!(cached.subject(), Some("Planning"));
        assert_eq!(cached.location(), None);
        assert_eq!(cached.original_start_time_zone(), Some("Custom/Source"));
    }

    #[test]
    fn incomplete_new_event_is_rejected_and_sensitive_debug_values_are_hidden() {
        let mut patch = CalendarOccurrencePatch::new();
        patch.set_subject(Some("Synthetic private subject".to_owned()));
        assert_eq!(
            patch.into_new(reference(), event_id()),
            Err(NewCalendarOccurrenceError::Incomplete)
        );

        let mut patch = CalendarOccurrencePatch::new();
        patch.set_subject(Some("Synthetic private subject".to_owned()));
        let mut time_patch = CalendarOccurrenceTimePatch::new();
        time_patch.set_start(date(2026, 10, 28).midnight().assume_utc());
        time_patch.set_end(date(2026, 10, 29).midnight().assume_utc());
        time_patch.set_is_all_day(true);
        patch.set_time(time_patch);
        let change = CalendarDeltaChange::Upsert {
            id: event_id(),
            patch,
        };
        let debug = format!("{change:?}");

        assert!(!debug.contains("Synthetic private subject"));
        assert!(!debug.contains("synthetic-event"));
        assert_eq!(CalendarField::<String>::Unchanged, CalendarField::default());

        let link = CalendarDeltaLink::from_opaque("https://example.test/secret-token".to_owned());
        assert_eq!(format!("{link:?}"), "CalendarDeltaLink(<redacted>)");
        assert_eq!(
            format!("{:?}", CalendarDeltaProgress::Complete(link)),
            "Complete(<redacted>)"
        );
    }

    #[test]
    fn invalid_new_event_time_is_reported_separately_from_missing_fields() {
        let mut patch = CalendarOccurrencePatch::new();
        let mut time_patch = CalendarOccurrenceTimePatch::new();
        time_patch.set_start(date(2026, 10, 29).midnight().assume_utc());
        time_patch.set_end(date(2026, 10, 28).midnight().assume_utc());
        time_patch.set_is_all_day(true);
        patch.set_time(time_patch);

        assert_eq!(
            patch.into_new(reference(), event_id()),
            Err(NewCalendarOccurrenceError::InvalidTime(
                InvalidCalendarOccurrenceTime
            ))
        );
    }
}
