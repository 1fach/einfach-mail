//! Provider-independent values for creating a single timed calendar event.

use std::fmt;

use thiserror::Error;
use time::{OffsetDateTime, PrimitiveDateTime};
use uuid::Uuid;

use super::subject::normalized_event_subject;
use crate::{CalendarOccurrenceTime, CalendarViewReference};

/// A stable identifier for one calendar event creation operation.
///
/// The value remains unchanged while the application recovers the same
/// unresolved creation. Providers use it in their recovery mechanisms.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CalendarEventCreationId(Uuid);

impl CalendarEventCreationId {
    /// Creates a random identifier for a new user-requested operation.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Parses a persisted operation identifier.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidCalendarEventCreationId`] when the value is not a UUID.
    pub fn parse(value: &str) -> Result<Self, InvalidCalendarEventCreationId> {
        Uuid::parse_str(value)
            .map(Self)
            .map_err(|_| InvalidCalendarEventCreationId)
    }

    /// Returns the UUID value used by the provider and encrypted cache.
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
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("calendar creation identifier must be a UUID")]
pub struct InvalidCalendarEventCreationId;

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
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("calendar event time zone must be a non-empty identifier")]
pub struct InvalidCalendarEventTimeZone;

/// A single timed event's source local schedule and resolved UTC range.
///
/// The source values preserve what the user entered. The UTC range is the
/// unambiguous instant range sent to the provider and used by the calendar cache.
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
    /// Retains `id` so the provider can identify the same operation during recovery.
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

    /// Returns the stable identifier used to recover this operation.
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
    /// The request may have reached the provider, or its accepted event is not yet cached.
    Unresolved(CalendarEventCreation),
    /// The provider rejected the request and only local marker cleanup remains.
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
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("calendar event endpoints must fit the selected view")]
pub struct InvalidCalendarEventCreation;

/// A calendar event draft contains invalid title or local-time values.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("calendar event draft must contain a title and ordered timed values")]
pub struct InvalidCalendarEventDraft;

#[cfg(test)]
mod tests;
