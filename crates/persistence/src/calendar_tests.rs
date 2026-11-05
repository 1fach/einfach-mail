use std::time::{Duration, UNIX_EPOCH};

use einfach_mail_application::{CalendarCache, CalendarCacheError};
use einfach_mail_domain::{
    AccountId, CalendarDeltaChange, CalendarDeltaLink, CalendarDeltaPage, CalendarDeltaProgress,
    CalendarOccurrencePatch, CalendarOccurrenceTime, CalendarOccurrenceTimePatch,
    CalendarViewRange, CalendarViewReference, EventId,
};
use tempfile::TempDir;
use time::{Date, Month, OffsetDateTime, Time};

use crate::store::SqliteMailCache;

const DATABASE_KEY_BYTE: u8 = 0x39;

fn account_id(identifier: &str) -> AccountId {
    AccountId::try_from(identifier.to_owned()).expect("synthetic account identifier is valid")
}

fn event_id() -> EventId {
    EventId::try_from("synthetic-calendar-event".to_owned())
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

async fn open_cache(directory: &TempDir, account: AccountId) -> SqliteMailCache {
    SqliteMailCache::open_with_test_key(
        directory.path().join("calendar.sqlite3"),
        account,
        DATABASE_KEY_BYTE,
    )
    .await
}

#[test]
fn calendar_cache_removal_preserves_the_in_use_error() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id("synthetic-account");
        let first = open_cache(&directory, account.clone()).await;
        let second = open_cache(&directory, account.clone()).await;

        assert_eq!(
            <SqliteMailCache as CalendarCache>::remove_account(&first, &account).await,
            Err(CalendarCacheError::InUse)
        );

        first.close_for_tests().await;
        second.close_for_tests().await;
    });
}

#[test]
fn calendar_cursor_and_sparse_event_updates_survive_restart_and_stay_range_scoped() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id("synthetic-account");
        let view = week(7);
        let next_link = link("synthetic-next-page");
        let first_page = CalendarDeltaPage::new(
            vec![CalendarDeltaChange::Upsert {
                id: event_id(),
                patch: all_day_event_patch(),
            }],
            CalendarDeltaProgress::Next(next_link.clone()),
        );
        let first_completion = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let cache = open_cache(&directory, account.clone()).await;
        cache
            .ensure_view(&view)
            .await
            .expect("synthetic view cursor is initialized");
        let state = cache
            .commit_delta_page(&view, first_page, first_completion)
            .await
            .expect("event row and resumable cursor commit together");
        assert_eq!(state.next_link(), Some(&next_link));
        assert!(state.delta_link().is_none());
        let events = cache
            .list_events(&view)
            .await
            .expect("synthetic event is cached");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].subject(), Some("Synthetic planning"));
        assert_eq!(events[0].location(), Some("Room A"));

        let mut patch = CalendarOccurrencePatch::new();
        patch.set_location(None);
        let mut time_patch = CalendarOccurrenceTimePatch::new();
        time_patch.set_start(utc_date(2030, Month::January, 9));
        patch.set_time(time_patch);
        let delta_link = link("synthetic-complete");
        let final_page = CalendarDeltaPage::new(
            vec![CalendarDeltaChange::Upsert {
                id: event_id(),
                patch,
            }],
            CalendarDeltaProgress::Complete(delta_link.clone()),
        );
        let completed_at = first_completion + Duration::from_secs(1);
        let state = cache
            .commit_delta_page(&view, final_page, completed_at)
            .await
            .expect("sparse event patch and completed cursor commit");
        assert_eq!(state.delta_link(), Some(&delta_link));
        assert!(state.next_link().is_none());
        assert_eq!(state.last_synchronized_at(), Some(completed_at));
        let events = cache
            .list_events(&view)
            .await
            .expect("sparse event update is readable");
        assert_eq!(events[0].subject(), Some("Synthetic planning"));
        assert_eq!(events[0].location(), None);
        assert_eq!(
            events[0].original_start_time_zone(),
            Some("Pacific Standard Time")
        );
        assert!(matches!(
            events[0].time(),
            CalendarOccurrenceTime::AllDay { start, end_exclusive }
                if *start == Date::from_calendar_date(2030, Month::January, 9).expect("valid date")
                    && *end_exclusive == Date::from_calendar_date(2030, Month::January, 10).expect("valid date")
        ));
        cache.close_for_tests().await;

        let reopened = open_cache(&directory, account.clone()).await;
        let reopened_state = reopened
            .ensure_view(&view)
            .await
            .expect("completed cursor survives a process restart");
        assert_eq!(reopened_state.delta_link(), Some(&delta_link));
        assert_eq!(reopened_state.last_synchronized_at(), Some(completed_at));
        assert_eq!(
            reopened
                .list_events(&view)
                .await
                .expect("cached event survives a process restart")[0]
                .subject(),
            Some("Synthetic planning")
        );
        assert!(
            reopened
                .list_events(&week(14))
                .await
                .expect("another fixed week has an independent cache scope")
                .is_empty()
        );
        let foreign_account_view =
            CalendarViewReference::new(account_id("other-account"), view.range().clone());
        assert_eq!(
            reopened.ensure_view(&foreign_account_view).await,
            Err(CalendarCacheError::AccountMismatch)
        );

        let (start, end) = timed_range(15, 9, 15, 10);
        let mut moved_patch = CalendarOccurrencePatch::new();
        let mut moved_time = CalendarOccurrenceTimePatch::new();
        moved_time.set_start(start);
        moved_time.set_end(end);
        moved_time.set_is_all_day(false);
        moved_patch.set_time(moved_time);
        reopened
            .commit_delta_page(
                &view,
                CalendarDeltaPage::new(
                    vec![CalendarDeltaChange::Upsert {
                        id: event_id(),
                        patch: moved_patch,
                    }],
                    CalendarDeltaProgress::Complete(link("synthetic-moved")),
                ),
                completed_at + Duration::from_secs(2),
            )
            .await
            .expect("an occurrence moved outside the view is removed from its cache");
        assert!(
            reopened
                .list_events(&view)
                .await
                .expect("events outside the fixed range are not returned")
                .is_empty()
        );
        reopened.close_for_tests().await;
    });
}

#[test]
fn malformed_delta_page_does_not_commit_events_or_advance_the_cursor() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id("synthetic-account");
        let view = week(7);
        let cache = open_cache(&directory, account).await;
        let original_link = link("synthetic-original");
        cache
            .commit_delta_page(
                &view,
                CalendarDeltaPage::new(
                    Vec::new(),
                    CalendarDeltaProgress::Next(original_link.clone()),
                ),
                UNIX_EPOCH + Duration::from_secs(1_800_000_000),
            )
            .await
            .expect("initial checkpoint commits");

        let mut incomplete_patch = CalendarOccurrencePatch::new();
        incomplete_patch.set_subject(Some("Incomplete synthetic event".to_owned()));
        let mut valid_patch = all_day_event_patch();
        valid_patch.set_subject(Some("Would roll back".to_owned()));
        let commit_result = cache
            .commit_delta_page(
                &view,
                CalendarDeltaPage::new(
                    vec![
                        CalendarDeltaChange::Upsert {
                            id: event_id(),
                            patch: valid_patch,
                        },
                        CalendarDeltaChange::Upsert {
                            id: EventId::try_from("incomplete-event".to_owned())
                                .expect("synthetic event ID is valid"),
                            patch: incomplete_patch,
                        },
                    ],
                    CalendarDeltaProgress::Complete(link("synthetic-invalid-page")),
                ),
                UNIX_EPOCH + Duration::from_secs(1_800_000_001),
            )
            .await;
        assert_eq!(commit_result, Err(CalendarCacheError::InvalidData));
        let state = cache
            .ensure_view(&view)
            .await
            .expect("prior checkpoint remains readable");
        assert_eq!(state.next_link(), Some(&original_link));
        assert!(
            cache
                .list_events(&view)
                .await
                .expect("prior event rows remain readable")
                .is_empty()
        );
        cache.close_for_tests().await;
    });
}
