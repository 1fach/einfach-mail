use std::time::{Duration, UNIX_EPOCH};

use einfach_mail_application::{CalendarCache, CalendarCacheError, CalendarDeltaRequest};
use einfach_mail_domain::{
    AccountId, CalendarDeltaChange, CalendarDeltaLink, CalendarDeltaPage, CalendarDeltaProgress,
    CalendarEventUpdate, CalendarEventUpdateDraft, CalendarEventVersion, CalendarOccurrencePatch,
    CalendarOccurrenceTime, CalendarOccurrenceTimePatch, CalendarViewRange, CalendarViewReference,
    EditableCalendarEvent, EventId,
};
use tempfile::TempDir;
use time::{Date, Month, OffsetDateTime, Time};

use crate::SqliteAccountCache;

const DATABASE_KEY_BYTE: u8 = 0x39;

fn account_id(identifier: &str) -> AccountId {
    AccountId::try_from(identifier.to_owned()).expect("synthetic account identifier is valid")
}

fn event_id() -> EventId {
    EventId::try_from("synthetic-calendar-event".to_owned())
        .expect("synthetic event identifier is valid")
}

fn removed_event_id() -> EventId {
    EventId::try_from("synthetic-removed-event".to_owned())
        .expect("synthetic event identifier is valid")
}

fn week(start_day: u8) -> CalendarViewReference {
    let start_date = Date::from_calendar_date(2030, Month::January, start_day)
        .expect("synthetic week start is valid");
    let end_date = start_date
        .checked_add(time::Duration::days(7))
        .expect("synthetic week end is valid");
    let range = CalendarViewRange::new(
        start_date.midnight().assume_utc(),
        end_date.midnight().assume_utc(),
        start_date,
        end_date,
    )
    .expect("synthetic week range is valid");
    CalendarViewReference::new(account_id("synthetic-account"), range)
}

fn all_day_event_patch() -> CalendarOccurrencePatch {
    let mut patch = CalendarOccurrencePatch::new();
    patch.set_subject(Some("Synthetic planning".to_owned()));
    patch.set_location(Some("Room A".to_owned()));
    patch.set_original_start_time_zone(Some("Pacific Standard Time".to_owned()));
    patch.set_original_end_time_zone(Some("Pacific Standard Time".to_owned()));
    let mut time = CalendarOccurrenceTimePatch::new();
    time.set_start(utc_date(2030, Month::January, 8));
    time.set_end(utc_date(2030, Month::January, 10));
    time.set_is_all_day(true);
    patch.set_time(time);
    patch
}

fn timed_event_patch(start: OffsetDateTime, end: OffsetDateTime) -> CalendarOccurrencePatch {
    let mut patch = CalendarOccurrencePatch::new();
    patch.set_subject(Some("Synthetic planning".to_owned()));
    let mut time = CalendarOccurrenceTimePatch::new();
    time.set_start(start);
    time.set_end(end);
    time.set_is_all_day(false);
    patch.set_time(time);
    patch
}

fn snapshot_event(
    view: &CalendarViewReference,
    identifier: &str,
    start_day: u8,
) -> einfach_mail_domain::CalendarOccurrence {
    let start = Date::from_calendar_date(2030, Month::January, start_day)
        .expect("synthetic snapshot event start date is valid")
        .with_time(Time::from_hms(9, 0, 0).expect("synthetic event start time is valid"))
        .assume_utc();
    let end = start + time::Duration::hours(1);
    timed_event_patch(start, end)
        .into_new(
            view.clone(),
            EventId::try_from(identifier.to_owned()).expect("synthetic event ID is valid"),
        )
        .expect("synthetic snapshot event is complete")
}

fn out_of_view_event_patch() -> CalendarOccurrencePatch {
    let mut patch = CalendarOccurrencePatch::new();
    let mut time = CalendarOccurrenceTimePatch::new();
    let (start, end) = timed_range(20, 9, 20, 10);
    time.set_start(start);
    time.set_end(end);
    time.set_is_all_day(false);
    patch.set_time(time);
    patch
}

fn utc_date(year: i32, month: Month, day: u8) -> OffsetDateTime {
    Date::from_calendar_date(year, month, day)
        .expect("synthetic date is valid")
        .midnight()
        .assume_utc()
}

fn link(cursor_token: &str) -> CalendarDeltaLink {
    CalendarDeltaLink::from_opaque(format!(
        "https://graph.microsoft.com/v1.0/me/calendarView/delta?$deltatoken={cursor_token}"
    ))
}

fn timed_range(
    start_day: u8,
    start_hour: u8,
    end_day: u8,
    end_hour: u8,
) -> (OffsetDateTime, OffsetDateTime) {
    let start = Date::from_calendar_date(2030, Month::January, start_day)
        .expect("synthetic event start date is valid")
        .with_time(Time::from_hms(start_hour, 0, 0).expect("synthetic start time is valid"))
        .assume_utc();
    let end = Date::from_calendar_date(2030, Month::January, end_day)
        .expect("synthetic event end date is valid")
        .with_time(Time::from_hms(end_hour, 0, 0).expect("synthetic end time is valid"))
        .assume_utc();
    (start, end)
}

async fn open_cache(directory: &TempDir, account: AccountId) -> SqliteAccountCache {
    SqliteAccountCache::open_with_test_key(
        directory.path().join("calendar.sqlite3"),
        account,
        DATABASE_KEY_BYTE,
    )
    .await
}

include!("sync.rs");
include!("event_writes.rs");
