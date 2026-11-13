use super::*;

#[test]
fn event_write_recovery_markers_survive_restart_and_clear_per_completed_view() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id("synthetic-account");
        let other_account = account_id("synthetic-other-account");
        let source_view = week(7);
        let other_view = week(14);
        let completed_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let cache = open_cache(&directory, account.clone()).await;
        assert_eq!(
            cache
                .account_has_calendar_write_sync_required(&other_account)
                .await,
            Err(CalendarCacheError::AccountMismatch)
        );
        assert_eq!(
            cache
                .clear_calendar_write_sync_required(&other_account)
                .await,
            Err(CalendarCacheError::AccountMismatch)
        );
        for (view, cursor) in [
            (&source_view, link("synthetic-source")),
            (&other_view, link("synthetic-other")),
        ] {
            cache
                .commit_delta_page(
                    view,
                    CalendarDeltaPage::new(Vec::new(), CalendarDeltaProgress::Complete(cursor)),
                    completed_at,
                )
                .await
                .expect("completed calendar view is stored");
        }
        cache
            .mark_calendar_write_sync_required(&source_view)
            .await
            .expect("event write marks every completed view");
        assert!(
            cache
                .account_has_calendar_write_sync_required(&account)
                .await
                .expect("write recovery state is readable")
        );
        cache.close_for_tests().await;

        let reopened = open_cache(&directory, account.clone()).await;
        assert!(
            reopened
                .account_has_calendar_write_sync_required(&account)
                .await
                .expect("write recovery state survives reopening")
        );
        reopened
            .commit_delta_page(
                &source_view,
                CalendarDeltaPage::new(
                    Vec::new(),
                    CalendarDeltaProgress::Next(link("synthetic-source-next")),
                ),
                completed_at + Duration::from_secs(1),
            )
            .await
            .expect("partial sync checkpoint commits");
        assert!(
            reopened
                .account_has_calendar_write_sync_required(&account)
                .await
                .expect("partial sync keeps the recovery gate")
        );
        reopened
            .commit_delta_page(
                &source_view,
                CalendarDeltaPage::new(
                    Vec::new(),
                    CalendarDeltaProgress::Complete(link("synthetic-source-reconciled")),
                ),
                completed_at + Duration::from_secs(2),
            )
            .await
            .expect("source view finishes reconciliation");
        assert!(
            reopened
                .account_has_calendar_write_sync_required(&account)
                .await
                .expect("the other view still requires reconciliation")
        );
        reopened
            .commit_delta_page(
                &other_view,
                CalendarDeltaPage::new(
                    Vec::new(),
                    CalendarDeltaProgress::Complete(link("synthetic-other-reconciled")),
                ),
                completed_at + Duration::from_secs(3),
            )
            .await
            .expect("other view finishes reconciliation");
        assert!(
            !reopened
                .account_has_calendar_write_sync_required(&account)
                .await
                .expect("all recovery markers clear after their view syncs")
        );
        reopened.close_for_tests().await;
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
        let editable = VersionedCalendarEvent::new(
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
            .mark_calendar_write_sync_required(&source_view)
            .await
            .expect("event update marks previously completed views");
        assert!(
            cache
                .account_has_calendar_write_sync_required(source_view.account_id())
                .await
                .expect("event write recovery state is readable")
        );

        cache
            .commit_event_update(&update, &updated_event)
            .await
            .expect("event rows move atomically to the overlapping view");
        assert!(
            !cache
                .account_has_calendar_write_sync_required(source_view.account_id())
                .await
                .expect("confirmed update clears recovery markers atomically")
        );
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
        let editable = VersionedCalendarEvent::new(
            event,
            CalendarEventVersion::try_from("synthetic-version-1".to_owned())
                .expect("synthetic event version is valid"),
        )
        .expect("synthetic event uses timed values");

        cache
            .mark_calendar_write_sync_required(&source_view)
            .await
            .expect("event deletion marks all completed views");
        assert!(
            cache
                .account_has_calendar_write_sync_required(source_view.account_id())
                .await
                .expect("event write recovery state is readable")
        );

        cache
            .commit_event_deletion(&editable)
            .await
            .expect("deletion removes every account view copy atomically");
        assert!(
            !cache
                .account_has_calendar_write_sync_required(source_view.account_id())
                .await
                .expect("confirmed deletion clears recovery markers atomically")
        );

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
        let editable = VersionedCalendarEvent::new(
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
        let editable = VersionedCalendarEvent::new(
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

        cache
            .mark_calendar_write_sync_required(&source_view)
            .await
            .expect("event update starts recovery tracking");

        assert_eq!(
            cache.commit_event_update(&update, &mismatched_event).await,
            Err(CalendarCacheError::InvalidData)
        );
        assert!(
            cache
                .account_has_calendar_write_sync_required(source_view.account_id())
                .await
                .expect("failed cache reconciliation keeps the recovery gate")
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
