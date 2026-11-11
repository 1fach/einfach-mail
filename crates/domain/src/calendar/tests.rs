use time::{Date, Duration, Month, OffsetDateTime};

use super::{
    CalendarDeltaChange, CalendarDeltaLink, CalendarDeltaProgress, CalendarField,
    CalendarOccurrence, CalendarOccurrencePatch, CalendarOccurrenceTime,
    CalendarOccurrenceTimePatch, CalendarViewRange, CalendarViewReference,
    InvalidCalendarOccurrenceTime, InvalidCalendarViewRange, NewCalendarOccurrenceError,
};
use crate::{AccountId, EventId};

fn account_id() -> AccountId {
    AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is not empty")
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
