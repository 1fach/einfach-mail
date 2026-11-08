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

use crate::store::SqliteAccountCache;

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

#[test]
fn confirmed_event_update_moves_the_cached_copy_between_overlapping_week_views() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let cache = open_cache(&directory, account_id("synthetic-account")).await;
        let source_view = week(7);
        let destination_view = week(14);
        cache
            .ensure_view(&destination_view)
            .await
            .expect("destination range is cached");
        let (start, end) = timed_range(13, 23, 14, 1);
        cache
            .commit_delta_page(
                &source_view,
                CalendarDeltaPage::new(
                    vec![CalendarDeltaChange::Upsert {
                        id: event_id(),
                        patch: timed_event_patch(start, end),
                    }],
                    CalendarDeltaProgress::Complete(link("synthetic-source")),
                ),
                UNIX_EPOCH + Duration::from_secs(1_800_000_000),
            )
            .await
            .expect("source event is cached");
        let event = cache
            .list_events(&source_view)
            .await
            .expect("source event is readable")
            .into_iter()
            .next()
            .expect("source event exists");
        let editable = EditableCalendarEvent::new(
            event,
            CalendarEventVersion::try_from("synthetic-version-1".to_owned())
                .expect("synthetic provider version is valid"),
        )
        .expect("synthetic event uses timed values");
        let (updated_start, updated_end) = timed_range(14, 9, 14, 10);
        let draft = CalendarEventUpdateDraft::timed(
            "Updated planning".to_owned(),
            updated_start,
            updated_end,
        )
        .expect("synthetic update is valid");
        let update = CalendarEventUpdate::new(editable, draft).expect("event values changed");
        let mut updated_event = update.source().occurrence().clone();
        let mut patch = CalendarOccurrencePatch::new();
        patch.set_subject(Some(update.draft().subject().to_owned()));
        let mut time = CalendarOccurrenceTimePatch::new();
        time.set_start(updated_start);
        time.set_end(updated_end);
        time.set_is_all_day(false);
        patch.set_time(time);
        patch
            .apply_to(&mut updated_event)
            .expect("synthetic update values are valid");

        cache
            .commit_event_update(&update, &updated_event)
            .await
            .expect("event rows move atomically to the overlapping view");
        assert!(
            cache
                .list_events(&source_view)
                .await
                .expect("old range is readable")
                .is_empty()
        );
        let destination_events = cache
            .list_events(&destination_view)
            .await
            .expect("new range is readable");
        assert_eq!(destination_events.len(), 1);
        assert_eq!(destination_events[0].subject(), Some("Updated planning"));
        assert_eq!(destination_events[0].reference(), &destination_view);
        assert_eq!(destination_events[0].time(), update.draft().time());
        cache.close_for_tests().await;
    });
}

#[test]
fn confirmed_event_deletion_removes_every_cached_copy_and_preserves_view_cursors() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let cache = open_cache(&directory, account_id("synthetic-account")).await;
        let source_view = week(7);
        let overlapping_view = week(14);
        let (start, end) = timed_range(13, 23, 14, 1);
        let source_link = link("synthetic-source");
        let overlapping_link = link("synthetic-overlap");
        for (view, cursor) in [
            (&source_view, &source_link),
            (&overlapping_view, &overlapping_link),
        ] {
            cache
                .commit_delta_page(
                    view,
                    CalendarDeltaPage::new(
                        vec![CalendarDeltaChange::Upsert {
                            id: event_id(),
                            patch: timed_event_patch(start, end),
                        }],
                        CalendarDeltaProgress::Complete(cursor.clone()),
                    ),
                    UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                )
                .await
                .expect("overlapping view and event are cached");
        }
        let event = cache
            .list_events(&source_view)
            .await
            .expect("source event is readable")
            .into_iter()
            .next()
            .expect("source event exists");
        let editable = EditableCalendarEvent::new(
            event,
            CalendarEventVersion::try_from("synthetic-version-1".to_owned())
                .expect("synthetic event version is valid"),
        )
        .expect("synthetic event uses timed values");

        cache
            .commit_event_deletion(&editable)
            .await
            .expect("deletion removes every account view copy atomically");

        for (view, cursor) in [
            (&source_view, &source_link),
            (&overlapping_view, &overlapping_link),
        ] {
            assert!(
                cache
                    .list_events(view)
                    .await
                    .expect("deleted event view remains readable")
                    .is_empty()
            );
            assert_eq!(
                cache
                    .ensure_view(view)
                    .await
                    .expect("deletion preserves the view cursor")
                    .delta_link(),
                Some(cursor)
            );
        }
        cache.close_for_tests().await;
    });
}

#[test]
fn event_deletion_without_a_cached_source_view_leaves_other_rows_unchanged() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let cache = open_cache(&directory, account_id("synthetic-account")).await;
        let source_view = week(7);
        let other_view = week(14);
        let (start, end) = timed_range(13, 23, 14, 1);
        cache
            .commit_delta_page(
                &other_view,
                CalendarDeltaPage::new(
                    vec![CalendarDeltaChange::Upsert {
                        id: event_id(),
                        patch: timed_event_patch(start, end),
                    }],
                    CalendarDeltaProgress::Complete(link("synthetic-other")),
                ),
                UNIX_EPOCH + Duration::from_secs(1_800_000_000),
            )
            .await
            .expect("other view and event are cached");
        let source_event = timed_event_patch(start, end)
            .into_new(source_view.clone(), event_id())
            .expect("synthetic source event is complete");
        let editable = EditableCalendarEvent::new(
            source_event,
            CalendarEventVersion::try_from("synthetic-version-1".to_owned())
                .expect("synthetic event version is valid"),
        )
        .expect("synthetic event uses timed values");
        assert_eq!(
            cache.commit_event_deletion(&editable).await,
            Err(CalendarCacheError::InvalidData)
        );
        let other_events = cache
            .list_events(&other_view)
            .await
            .expect("other event remains readable");
        assert_eq!(other_events.len(), 1);
        assert_eq!(other_events[0].id(), &event_id());
        cache.close_for_tests().await;
    });
}

#[test]
fn invalid_event_update_leaves_all_cached_copies_unchanged() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let cache = open_cache(&directory, account_id("synthetic-account")).await;
        let source_view = week(7);
        let destination_view = week(14);
        let (start, end) = timed_range(13, 23, 14, 1);
        for view in [&source_view, &destination_view] {
            cache
                .commit_delta_page(
                    view,
                    CalendarDeltaPage::new(
                        vec![CalendarDeltaChange::Upsert {
                            id: event_id(),
                            patch: timed_event_patch(start, end),
                        }],
                        CalendarDeltaProgress::Complete(link("synthetic-source")),
                    ),
                    UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                )
                .await
                .expect("synthetic event is cached in the supplied range");
        }
        let original_start = cache
            .list_events(&source_view)
            .await
            .expect("source range is readable")[0]
            .clone();
        let editable = EditableCalendarEvent::new(
            original_start,
            CalendarEventVersion::try_from("synthetic-version-1".to_owned())
                .expect("synthetic provider version is valid"),
        )
        .expect("synthetic event uses timed values");
        let draft = CalendarEventUpdateDraft::timed(
            "Updated planning".to_owned(),
            timed_range(14, 9, 14, 10).0,
            timed_range(14, 9, 14, 10).1,
        )
        .expect("synthetic update is valid");
        let update = CalendarEventUpdate::new(editable, draft).expect("event values changed");
        let mut mismatched_event = update.source().occurrence().clone();
        let mut patch = CalendarOccurrencePatch::new();
        patch.set_subject(Some("Unexpected event".to_owned()));
        patch
            .apply_to(&mut mismatched_event)
            .expect("subject update is valid");

        assert_eq!(
            cache.commit_event_update(&update, &mismatched_event).await,
            Err(CalendarCacheError::InvalidData)
        );
        let source_events = cache
            .list_events(&source_view)
            .await
            .expect("source range remains readable");
        assert_eq!(source_events.len(), 1);
        assert_eq!(source_events[0].subject(), Some("Synthetic planning"));
        assert_eq!(
            source_events[0].time(),
            &CalendarOccurrenceTime::Timed { start, end }
        );
        let destination_events = cache
            .list_events(&destination_view)
            .await
            .expect("destination range remains readable");
        assert_eq!(destination_events.len(), 1);
        assert_eq!(destination_events[0].subject(), Some("Synthetic planning"));
        cache.close_for_tests().await;
    });
}

#[test]
fn expired_cursor_replacement_preserves_snapshot_until_atomic_completion() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id("synthetic-account");
        let view = week(7);
        let cache = open_cache(&directory, account.clone()).await;
        let completed_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let old_delta = link("synthetic-old-delta");
        cache
            .commit_delta_page(
                &view,
                CalendarDeltaPage::new(
                    vec![
                        CalendarDeltaChange::Upsert {
                            id: event_id(),
                            patch: all_day_event_patch(),
                        },
                        CalendarDeltaChange::Upsert {
                            id: removed_event_id(),
                            patch: all_day_event_patch(),
                        },
                    ],
                    CalendarDeltaProgress::Complete(old_delta.clone()),
                ),
                completed_at,
            )
            .await
            .expect("initial calendar snapshot and cursor commit");

        let reset_link = link("synthetic-reset");
        let state = cache
            .begin_full_resync(&view, Some(reset_link.clone()))
            .await
            .expect("replacement scan starts without changing the active snapshot");
        assert!(state.replacement_active());
        assert!(
            cache
                .account_has_active_replacement(&account)
                .await
                .expect("account replacement state is readable")
        );
        assert_eq!(state.delta_link(), Some(&old_delta));
        assert_eq!(state.next_link(), Some(&reset_link));
        assert_eq!(state.last_synchronized_at(), Some(completed_at));
        assert_eq!(
            cache
                .list_events(&view)
                .await
                .expect("active snapshot remains readable")
                .len(),
            2
        );
        cache.close_for_tests().await;

        let reopened = open_cache(&directory, account.clone()).await;
        let persisted = reopened
            .ensure_view(&view)
            .await
            .expect("replacement checkpoint survives restart");
        assert!(persisted.replacement_active());
        assert_eq!(
            persisted.request(),
            CalendarDeltaRequest::Continue(reset_link)
        );

        let mut sparse_update = CalendarOccurrencePatch::new();
        sparse_update.set_subject(Some("Updated during replacement".to_owned()));
        let next_link = link("synthetic-replacement-next");
        let replacement_progress = reopened
            .commit_delta_page(
                &view,
                CalendarDeltaPage::new(
                    vec![
                        CalendarDeltaChange::Upsert {
                            id: event_id(),
                            patch: sparse_update,
                        },
                        CalendarDeltaChange::Removed {
                            id: removed_event_id(),
                        },
                        CalendarDeltaChange::Upsert {
                            id: EventId::try_from("synthetic-out-of-view".to_owned())
                                .expect("synthetic event identifier is valid"),
                            patch: out_of_view_event_patch(),
                        },
                    ],
                    CalendarDeltaProgress::Next(next_link.clone()),
                ),
                completed_at + Duration::from_secs(1),
            )
            .await
            .expect("replacement events and next link commit together");
        assert!(replacement_progress.replacement_active());
        assert_eq!(replacement_progress.delta_link(), Some(&old_delta));
        assert_eq!(replacement_progress.next_link(), Some(&next_link));
        assert_eq!(
            reopened
                .list_events(&view)
                .await
                .expect("old snapshot stays visible during replacement")
                .len(),
            2
        );
        reopened.close_for_tests().await;

        let reopened = open_cache(&directory, account.clone()).await;
        let persisted = reopened
            .ensure_view(&view)
            .await
            .expect("staged replacement page survives restart");
        assert!(persisted.replacement_active());
        assert_eq!(
            persisted.request(),
            CalendarDeltaRequest::Continue(next_link.clone())
        );

        let mut removed_event_patch = CalendarOccurrencePatch::new();
        removed_event_patch.set_subject(Some("Must not restore removed event".to_owned()));
        let rejected_sparse_update = reopened
            .commit_delta_page(
                &view,
                CalendarDeltaPage::new(
                    vec![CalendarDeltaChange::Upsert {
                        id: removed_event_id(),
                        patch: removed_event_patch,
                    }],
                    CalendarDeltaProgress::Next(link("synthetic-uncommitted-next")),
                ),
                completed_at + Duration::from_secs(2),
            )
            .await;
        assert_eq!(rejected_sparse_update, Err(CalendarCacheError::InvalidData));
        let state = reopened
            .ensure_view(&view)
            .await
            .expect("rejected page leaves replacement checkpoint intact");
        assert_eq!(state.next_link(), Some(&next_link));
        assert_eq!(
            reopened
                .list_events(&view)
                .await
                .expect("rejected page leaves old snapshot visible")
                .len(),
            2
        );

        let new_delta = link("synthetic-new-delta");
        let completed_at = completed_at + Duration::from_secs(3);
        let state = reopened
            .commit_delta_page(
                &view,
                CalendarDeltaPage::new(
                    Vec::new(),
                    CalendarDeltaProgress::Complete(new_delta.clone()),
                ),
                completed_at,
            )
            .await
            .expect("final replacement page atomically installs the snapshot and cursor");
        assert!(!state.replacement_active());
        assert_eq!(state.delta_link(), Some(&new_delta));
        assert!(state.next_link().is_none());
        assert_eq!(state.last_synchronized_at(), Some(completed_at));
        assert!(
            !reopened
                .account_has_active_replacement(&account)
                .await
                .expect("completed replacement state is readable")
        );
        let events = reopened
            .list_events(&view)
            .await
            .expect("replacement snapshot is readable after completion");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id(), &event_id());
        assert_eq!(events[0].subject(), Some("Updated during replacement"));
        reopened.close_for_tests().await;
    });
}
