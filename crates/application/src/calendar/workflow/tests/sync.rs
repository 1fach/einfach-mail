use std::{sync::Arc, time::UNIX_EPOCH};

use einfach_mail_domain::{
    CalendarDeltaChange, CalendarDeltaProgress, CalendarOccurrencePatch, CalendarOccurrenceTime,
    CalendarOccurrenceTimePatch,
};
use time::{Date, Month};

use crate::shared::sync_budget::MAX_SYNC_RESPONSE_BYTES;
use crate::test_support::{
    FixtureCache, FixtureProvider, all_day_patch, event_id, link, replacement_event_id, response,
    utc, view, workflow_account_id,
};
use crate::{
    CalendarCache, CalendarCacheError, CalendarDeltaRequest, CalendarOperationError,
    CalendarViewSnapshotResponse, CalendarWorkflow, CalendarWorkflowError, CalendarWriteReadiness,
};

#[test]
fn snapshot_cache_replaces_events_and_rejects_duplicate_ids() {
    smol::block_on(async {
        let view = view();
        let patch = all_day_patch(Some("Synthetic snapshot"), None);
        let occurrence = patch
            .into_new(view.clone(), replacement_event_id())
            .expect("synthetic snapshot event is complete");
        let cache = FixtureCache::default();
        let completed_at = UNIX_EPOCH + std::time::Duration::from_secs(1_800_000_000);

        let state = cache
            .replace_snapshot(&view, vec![occurrence.clone()], completed_at)
            .await
            .expect("complete snapshot replaces cached events");
        assert!(state.is_snapshot());
        assert!(state.delta_link().is_none());
        assert_eq!(state.last_synchronized_at(), Some(completed_at));
        assert_eq!(
            cache
                .list_events(&view)
                .await
                .expect("snapshot is readable"),
            vec![occurrence.clone()]
        );

        assert_eq!(
            cache
                .replace_snapshot(&view, vec![occurrence.clone(), occurrence], completed_at)
                .await,
            Err(CalendarCacheError::InvalidData)
        );
        assert_eq!(
            cache
                .list_events(&view)
                .await
                .expect("failed snapshot keeps cache"),
            vec![
                all_day_patch(Some("Synthetic snapshot"), None)
                    .into_new(view.clone(), replacement_event_id())
                    .expect("synthetic snapshot event remains complete")
            ]
        );

        cache
            .replace_snapshot(&view, Vec::new(), completed_at)
            .await
            .expect("an empty complete snapshot clears its view");
        assert!(
            cache
                .list_events(&view)
                .await
                .expect("empty snapshot is readable")
                .is_empty()
        );
    });
}

#[test]
fn sync_commits_pages_and_preserves_omitted_fields() {
    smol::block_on(async {
        let view = view();
        let next_link = link("synthetic-next");
        let delta_link = link("synthetic-complete");
        let provider = FixtureProvider::with_pages([
            Ok(response(
                vec![CalendarDeltaChange::Upsert {
                    id: event_id(),
                    patch: all_day_patch(Some("Synthetic appointment"), Some("Room 1")),
                }],
                CalendarDeltaProgress::Next(next_link.clone()),
            )),
            Ok(response(
                vec![CalendarDeltaChange::Upsert {
                    id: event_id(),
                    patch: {
                        let mut patch = CalendarOccurrencePatch::new();
                        patch.set_location(None);
                        let mut time = CalendarOccurrenceTimePatch::new();
                        time.set_start(utc(2030, Month::January, 9));
                        patch.set_time(time);
                        patch
                    },
                }],
                CalendarDeltaProgress::Complete(delta_link.clone()),
            )),
        ]);
        let request_log = Arc::clone(&provider.requests);
        let workflow =
            CalendarWorkflow::new(workflow_account_id(), provider, FixtureCache::default());

        let report = workflow.sync_view(&view).await.expect("both pages commit");
        assert_eq!(report.response_count(), 2);
        assert_eq!(
            *request_log
                .lock()
                .expect("synthetic request log mutex is not poisoned"),
            vec![
                CalendarDeltaRequest::Initial,
                CalendarDeltaRequest::Continue(next_link)
            ]
        );
        let snapshot = workflow
            .load_cached_view(&view)
            .await
            .expect("synthetic snapshot is readable");
        assert_eq!(snapshot.sync_state().delta_link(), Some(&delta_link));
        assert!(snapshot.sync_state().next_link().is_none());
        assert_eq!(snapshot.events().len(), 1);
        assert_eq!(
            snapshot.events()[0].subject(),
            Some("Synthetic appointment")
        );
        assert_eq!(snapshot.events()[0].location(), None);
        assert!(matches!(
            snapshot.events()[0].time(),
            CalendarOccurrenceTime::AllDay { start, end_exclusive }
                if *start == Date::from_calendar_date(2030, Month::January, 9).expect("valid date")
                    && *end_exclusive == Date::from_calendar_date(2030, Month::January, 10).expect("valid date")
        ));
    });
}

#[test]
fn snapshot_strategy_commits_one_response_without_requesting_delta_pages() {
    smol::block_on(async {
        let view = view();
        let occurrence = all_day_patch(Some("Synthetic snapshot"), None)
            .into_new(view.clone(), replacement_event_id())
            .expect("synthetic snapshot event is complete");
        let provider = FixtureProvider::with_snapshots([Ok(CalendarViewSnapshotResponse::new(
            vec![occurrence.clone()],
            256,
        ))]);
        let requests = Arc::clone(&provider.requests);
        let workflow =
            CalendarWorkflow::new(workflow_account_id(), provider, FixtureCache::default());

        let report = workflow
            .sync_view(&view)
            .await
            .expect("the complete provider snapshot commits");
        assert_eq!(report.response_count(), 1);
        assert!(
            requests
                .lock()
                .expect("synthetic request log mutex is not poisoned")
                .is_empty()
        );

        let cached = workflow
            .load_cached_view(&view)
            .await
            .expect("the committed snapshot is readable");
        assert!(cached.sync_state().is_snapshot());
        assert!(cached.sync_state().delta_link().is_none());
        assert!(cached.sync_state().last_synchronized_at().is_some());
        assert_eq!(cached.events(), &[occurrence]);
        assert_eq!(cached.write_readiness(), CalendarWriteReadiness::Ready);
    });
}

#[test]
fn snapshot_sync_holds_the_view_reservation_until_cache_commit() {
    smol::block_on(async {
        let view = view();
        let provider =
            FixtureProvider::with_snapshots([Ok(CalendarViewSnapshotResponse::new(Vec::new(), 0))]);
        let (started, release) = provider.hold_next_snapshot();
        let workflow = std::sync::Arc::new(CalendarWorkflow::new(
            workflow_account_id(),
            provider,
            FixtureCache::default(),
        ));
        let syncing_workflow = std::sync::Arc::clone(&workflow);
        let syncing_view = view.clone();
        let sync = smol::spawn(async move { syncing_workflow.sync_view(&syncing_view).await });
        started
            .await
            .expect("the synthetic snapshot request has started");

        assert_eq!(
            workflow.sync_view(&view).await,
            Err(CalendarWorkflowError::OperationInProgress)
        );

        release
            .send(())
            .expect("the synthetic snapshot request is waiting for release");
        assert_eq!(
            sync.await
                .expect("the snapshot sync task completes")
                .response_count(),
            1
        );
    });
}

#[test]
fn snapshot_provider_and_budget_failures_preserve_cached_view_and_release_reservation() {
    smol::block_on(async {
        let view = view();
        let prior_event = all_day_patch(Some("Prior snapshot"), None)
            .into_new(view.clone(), event_id())
            .expect("synthetic prior snapshot event is complete");
        let replacement_event = all_day_patch(Some("Replacement snapshot"), None)
            .into_new(view.clone(), replacement_event_id())
            .expect("synthetic replacement snapshot event is complete");
        let completed_at = UNIX_EPOCH + std::time::Duration::from_secs(1_800_000_000);
        let cache = FixtureCache::default();
        cache
            .replace_snapshot(&view, vec![prior_event.clone()], completed_at)
            .await
            .expect("prior snapshot is cached");
        let provider = FixtureProvider::with_snapshots([
            Err(CalendarOperationError::ServiceUnavailable),
            Ok(CalendarViewSnapshotResponse::new(
                vec![replacement_event],
                MAX_SYNC_RESPONSE_BYTES + 1,
            )),
        ]);
        let requests = Arc::clone(&provider.requests);
        let workflow = CalendarWorkflow::new(workflow_account_id(), provider, cache);

        assert_eq!(
            workflow.sync_view(&view).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::ServiceUnavailable
            ))
        );
        assert_eq!(
            workflow.sync_view(&view).await,
            Err(CalendarWorkflowError::ResponseLimitExceeded)
        );
        assert!(
            requests
                .lock()
                .expect("synthetic request log mutex is not poisoned")
                .is_empty()
        );

        let cached = workflow
            .load_cached_view(&view)
            .await
            .expect("the prior snapshot remains readable");
        assert!(cached.sync_state().is_snapshot());
        assert_eq!(
            cached.sync_state().last_synchronized_at(),
            Some(completed_at)
        );
        assert_eq!(cached.events(), &[prior_event]);
        assert_eq!(
            cached.write_readiness(),
            CalendarWriteReadiness::ViewSyncRequired
        );
    });
}

#[test]
fn failed_snapshot_cache_commit_does_not_mark_view_synchronized() {
    smol::block_on(async {
        let view = view();
        let prior_event = all_day_patch(Some("Prior snapshot"), None)
            .into_new(view.clone(), event_id())
            .expect("synthetic prior snapshot event is complete");
        let replacement_event = all_day_patch(Some("Replacement snapshot"), None)
            .into_new(view.clone(), replacement_event_id())
            .expect("synthetic replacement snapshot event is complete");
        let completed_at = UNIX_EPOCH + std::time::Duration::from_secs(1_800_000_000);
        let cache = FixtureCache::default();
        cache
            .replace_snapshot(&view, vec![prior_event.clone()], completed_at)
            .await
            .expect("prior snapshot is cached");
        cache.fail_snapshot_commit(CalendarCacheError::Unavailable);
        let provider = FixtureProvider::with_snapshots([Ok(CalendarViewSnapshotResponse::new(
            vec![replacement_event],
            256,
        ))]);
        let workflow = CalendarWorkflow::new(workflow_account_id(), provider, cache);

        assert_eq!(
            workflow.sync_view(&view).await,
            Err(CalendarWorkflowError::Cache(
                CalendarCacheError::Unavailable
            ))
        );

        let cached = workflow
            .load_cached_view(&view)
            .await
            .expect("the prior view state remains readable");
        assert!(cached.sync_state().is_snapshot());
        assert_eq!(
            cached.sync_state().last_synchronized_at(),
            Some(completed_at)
        );
        assert_eq!(cached.events(), &[prior_event]);
        assert_eq!(
            cached.write_readiness(),
            CalendarWriteReadiness::ViewSyncRequired
        );
    });
}

#[test]
fn interrupted_sync_resumes_the_saved_next_link() {
    smol::block_on(async {
        let view = view();
        let next_link = link("synthetic-resume");
        let delta_link = link("synthetic-final");
        let provider = FixtureProvider::with_pages([
            Ok(response(
                vec![CalendarDeltaChange::Upsert {
                    id: event_id(),
                    patch: all_day_patch(Some("Synthetic appointment"), None),
                }],
                CalendarDeltaProgress::Next(next_link.clone()),
            )),
            Err(CalendarOperationError::ServiceUnavailable),
        ]);
        let request_log = Arc::clone(&provider.requests);
        let cache = FixtureCache::default();
        let workflow = CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache);

        assert_eq!(
            workflow.sync_view(&view).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::ServiceUnavailable
            ))
        );
        provider.push_page(Ok(response(
            Vec::new(),
            CalendarDeltaProgress::Complete(delta_link.clone()),
        )));
        workflow
            .sync_view(&view)
            .await
            .expect("the interrupted cursor resumes");

        assert_eq!(
            *request_log
                .lock()
                .expect("synthetic request log mutex is not poisoned"),
            vec![
                CalendarDeltaRequest::Initial,
                CalendarDeltaRequest::Continue(next_link.clone()),
                CalendarDeltaRequest::Continue(next_link),
            ]
        );
        let snapshot = workflow
            .load_cached_view(&view)
            .await
            .expect("synthetic snapshot is readable");
        assert_eq!(snapshot.sync_state().delta_link(), Some(&delta_link));
        assert!(snapshot.sync_state().next_link().is_none());
        assert_eq!(snapshot.events().len(), 1);
    });
}

#[test]
fn repeated_delta_cursor_stops_before_committing_the_looping_page() {
    smol::block_on(async {
        let view = view();
        let repeated_link = link("synthetic-repeated");
        let provider = FixtureProvider::with_pages([
            Ok(response(
                Vec::new(),
                CalendarDeltaProgress::Next(repeated_link.clone()),
            )),
            Ok(response(
                Vec::new(),
                CalendarDeltaProgress::Next(repeated_link),
            )),
        ]);
        let workflow =
            CalendarWorkflow::new(workflow_account_id(), provider, FixtureCache::default());

        assert_eq!(
            workflow.sync_view(&view).await,
            Err(CalendarWorkflowError::RepeatedCursor)
        );
        let snapshot = workflow
            .load_cached_view(&view)
            .await
            .expect("synthetic cursor state is readable");
        assert!(snapshot.sync_state().next_link().is_some());
    });
}

#[test]
fn a_completed_cursor_remains_available_for_the_next_round() {
    smol::block_on(async {
        let view = view();
        let delta_link = link("synthetic-delta");
        let provider = FixtureProvider::with_pages([Ok(response(
            Vec::new(),
            CalendarDeltaProgress::Complete(delta_link.clone()),
        ))]);
        let workflow = CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        );

        workflow
            .sync_view(&view)
            .await
            .expect("initial round completes");
        provider.push_page(Ok(response(
            Vec::new(),
            CalendarDeltaProgress::Complete(link("synthetic-next-round")),
        )));
        workflow
            .sync_view(&view)
            .await
            .expect("following round completes");

        assert_eq!(
            *provider
                .requests
                .lock()
                .expect("synthetic request log mutex is not poisoned"),
            vec![
                CalendarDeltaRequest::Initial,
                CalendarDeltaRequest::Continue(delta_link)
            ]
        );
    });
}
