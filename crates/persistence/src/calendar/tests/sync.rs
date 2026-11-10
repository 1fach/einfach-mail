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
fn complete_calendar_snapshots_are_exact_scoped_and_survive_reopen() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id("synthetic-account");
        let view = week(7);
        let sibling_view = week(14);
        let event = snapshot_event(&view, "synthetic-view-event", 8);
        let sibling_event = snapshot_event(&sibling_view, "synthetic-sibling-event", 15);
        let completed_at = UNIX_EPOCH + Duration::from_secs(1_900_000_000);
        let cache = open_cache(&directory, account.clone()).await;

        let state = cache
            .replace_snapshot(&view, vec![event.clone()], completed_at)
            .await
            .expect("complete snapshot replaces one fixed view");
        assert!(state.is_snapshot());
        assert!(state.delta_link().is_none());
        assert!(state.next_link().is_none());
        assert_eq!(state.last_synchronized_at(), Some(completed_at));
        assert_eq!(
            cache
                .replace_snapshot(&view, vec![event.clone(), event.clone()], completed_at)
                .await,
            Err(CalendarCacheError::InvalidData)
        );
        assert_eq!(
            cache
                .list_events(&view)
                .await
                .expect("rejected duplicate keeps the prior snapshot"),
            vec![event.clone()]
        );
        cache
            .replace_snapshot(&sibling_view, vec![sibling_event.clone()], completed_at)
            .await
            .expect("sibling snapshot has independent state");
        cache.close_for_tests().await;

        let reopened = open_cache(&directory, account.clone()).await;
        let persisted = reopened
            .ensure_view(&view)
            .await
            .expect("snapshot completion survives restart");
        assert!(persisted.is_snapshot());
        assert_eq!(persisted.last_synchronized_at(), Some(completed_at));
        assert_eq!(
            reopened
                .list_events(&view)
                .await
                .expect("snapshot events are readable"),
            vec![event]
        );
        assert_eq!(
            reopened
                .list_events(&sibling_view)
                .await
                .expect("sibling snapshot remains readable"),
            vec![sibling_event.clone()]
        );

        let next_link = link("synthetic-graph-next");
        let graph_state = reopened
            .commit_delta_page(
                &view,
                CalendarDeltaPage::new(Vec::new(), CalendarDeltaProgress::Next(next_link.clone())),
                completed_at + Duration::from_secs(1),
            )
            .await
            .expect("Graph progress supersedes snapshot state for this view");
        assert!(!graph_state.is_snapshot());
        assert_eq!(graph_state.next_link(), Some(&next_link));
        assert!(graph_state.last_synchronized_at().is_none());

        let empty_completed_at = completed_at + Duration::from_secs(2);
        let empty_state = reopened
            .replace_snapshot(&view, Vec::new(), empty_completed_at)
            .await
            .expect("empty complete snapshot clears only this view");
        assert!(empty_state.is_snapshot());
        assert!(
            reopened
                .list_events(&view)
                .await
                .expect("empty snapshot has no cached events")
                .is_empty()
        );
        let foreign_view =
            CalendarViewReference::new(account_id("other-account"), view.range().clone());
        let foreign_event = snapshot_event(&foreign_view, "synthetic-foreign-event", 8);
        assert_eq!(
            reopened
                .replace_snapshot(&view, vec![foreign_event], empty_completed_at)
                .await,
            Err(CalendarCacheError::AccountMismatch)
        );
        reopened.close_for_tests().await;

        let reopened = open_cache(&directory, account).await;
        let persisted_empty = reopened
            .ensure_view(&view)
            .await
            .expect("empty snapshot state survives restart");
        assert!(persisted_empty.is_snapshot());
        assert_eq!(
            persisted_empty.last_synchronized_at(),
            Some(empty_completed_at)
        );
        assert!(
            reopened
                .list_events(&view)
                .await
                .expect("empty snapshot remains empty after restart")
                .is_empty()
        );
        assert_eq!(
            reopened
                .ensure_view(&sibling_view)
                .await
                .expect("sibling snapshot state remains independent")
                .last_synchronized_at(),
            Some(completed_at)
        );
        assert_eq!(
            reopened
                .list_events(&sibling_view)
                .await
                .expect("sibling snapshot remains after another view clears"),
            vec![sibling_event]
        );
        reopened.close_for_tests().await;
    });
}

#[test]
fn failed_calendar_snapshot_commit_preserves_events_and_graph_checkpoint() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id("synthetic-account");
        let view = week(7);
        let prior_event = snapshot_event(&view, "synthetic-prior-event", 8);
        let next_event = snapshot_event(&view, "synthetic-new-event", 9);
        let delta_link = link("synthetic-prior-delta");
        let completed_at = UNIX_EPOCH + Duration::from_secs(1_900_000_100);
        let cache = open_cache(&directory, account.clone()).await;
        let prior_start = Date::from_calendar_date(2030, Month::January, 8)
            .expect("synthetic event date is valid")
            .with_time(Time::from_hms(9, 0, 0).expect("valid time"))
            .assume_utc();
        cache
            .commit_delta_page(
                &view,
                CalendarDeltaPage::new(
                    vec![CalendarDeltaChange::Upsert {
                        id: prior_event.id().clone(),
                        patch: timed_event_patch(
                            prior_start,
                            prior_start + time::Duration::hours(1),
                        ),
                    }],
                    CalendarDeltaProgress::Complete(delta_link.clone()),
                ),
                completed_at,
            )
            .await
            .expect("prior Graph state is complete");
        cache
            .execute_batch_for_tests(
                "CREATE TRIGGER fail_calendar_snapshot_state_insert
                 BEFORE INSERT ON calendar_view_snapshot_state
                 BEGIN SELECT RAISE(ABORT, 'synthetic snapshot failure'); END;",
            )
            .await
            .expect("synthetic rollback trigger is installed");

        assert_eq!(
            cache
                .replace_snapshot(
                    &view,
                    vec![next_event],
                    completed_at + Duration::from_secs(1),
                )
                .await,
            Err(CalendarCacheError::Unavailable)
        );
        let state = cache
            .ensure_view(&view)
            .await
            .expect("failed replacement preserves prior sync state");
        assert!(!state.is_snapshot());
        assert_eq!(state.delta_link(), Some(&delta_link));
        assert_eq!(state.last_synchronized_at(), Some(completed_at));
        assert_eq!(
            cache
                .list_events(&view)
                .await
                .expect("prior event remains readable"),
            vec![prior_event.clone()]
        );

        cache
            .execute_batch_for_tests("DROP TRIGGER fail_calendar_snapshot_state_insert;")
            .await
            .expect("synthetic rollback trigger is removed");
        cache.close_for_tests().await;

        let reopened = open_cache(&directory, account).await;
        let persisted = reopened
            .ensure_view(&view)
            .await
            .expect("prior Graph checkpoint survives restart after rollback");
        assert_eq!(persisted.delta_link(), Some(&delta_link));
        assert_eq!(persisted.last_synchronized_at(), Some(completed_at));
        assert_eq!(
            reopened
                .list_events(&view)
                .await
                .expect("prior event survives rollback and restart"),
            vec![prior_event]
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
