use einfach_mail_domain::{
    CalendarDeltaChange, CalendarDeltaProgress, CalendarEventUpdate, CalendarEventUpdateDraft,
    CalendarOccurrencePatch,
};
use time::Month;

use crate::test_support::{
    FixtureCache, FixtureProvider, creation, cross_week_timed_event_patch, event_id, link,
    response, synchronize_empty_view, utc, view, view_for_week, workflow_account_id,
};
use crate::{
    CalendarEventCreationOutcome, CalendarEventUpdateOutcome, CalendarOperationError,
    CalendarWorkflow, CalendarWorkflowError, CalendarWriteReadiness,
};

#[test]
fn uncertain_update_requires_sync_for_each_previously_synced_account_view() {
    smol::block_on(async {
        let source_view = view();
        let overlapping_view = view_for_week(14);
        let provider = FixtureProvider::default();
        let workflow = CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        );

        for (view, cursor) in [
            (&source_view, "cross-week-source-initial"),
            (&overlapping_view, "cross-week-overlap-initial"),
        ] {
            provider.push_page(Ok(response(
                vec![CalendarDeltaChange::Upsert {
                    id: event_id(),
                    patch: cross_week_timed_event_patch(),
                }],
                CalendarDeltaProgress::Complete(link(cursor)),
            )));
            workflow
                .sync_view(view)
                .await
                .expect("each synthetic view completes its initial sync");
        }

        let source_event = workflow
            .load_cached_view(&source_view)
            .await
            .expect("source event view is readable")
            .events()[0]
            .clone();
        let overlapping_event = workflow
            .load_cached_view(&overlapping_view)
            .await
            .expect("overlapping event view is readable")
            .events()[0]
            .clone();
        let editable = workflow
            .load_versioned_event(&source_event)
            .await
            .expect("synchronized event can be opened for editing");
        let draft = CalendarEventUpdateDraft::timed(
            "Updated cross-week appointment".to_owned(),
            utc(2030, Month::January, 13) + time::Duration::hours(9),
            utc(2030, Month::January, 15) + time::Duration::hours(10),
        )
        .expect("cross-week edit draft is valid");
        let update = CalendarEventUpdate::new(editable, draft).expect("the subject changed");
        provider.fail_next_update(CalendarOperationError::UpdateOutcomeUnknown);

        assert_eq!(
            workflow.update_event(&update).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::UpdateOutcomeUnknown
            ))
        );
        for view in [&source_view, &overlapping_view] {
            assert!(
                workflow
                    .load_cached_view(view)
                    .await
                    .expect("each cached event view is readable")
                    .write_readiness()
                    == CalendarWriteReadiness::AccountSyncRequired
            );
        }
        assert_eq!(
            workflow.load_versioned_event(&overlapping_event).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );

        for (view, cursor) in [
            (&source_view, "cross-week-source-recovered"),
            (&overlapping_view, "cross-week-overlap-recovered"),
        ] {
            let mut patch = CalendarOccurrencePatch::new();
            patch.set_subject(Some("Updated cross-week appointment".to_owned()));
            provider.push_page(Ok(response(
                vec![CalendarDeltaChange::Upsert {
                    id: event_id(),
                    patch,
                }],
                CalendarDeltaProgress::Complete(link(cursor)),
            )));
            workflow
                .sync_view(view)
                .await
                .expect("each view independently reconciles the appointment");
            assert_eq!(
                workflow
                    .load_cached_view(view)
                    .await
                    .expect("reconciled view is readable")
                    .write_readiness(),
                if view == &source_view {
                    CalendarWriteReadiness::AccountSyncRequired
                } else {
                    CalendarWriteReadiness::Ready
                },
                "every account view stays gated until the last recovery sync completes"
            );
            if view == &source_view {
                assert!(
                    workflow
                        .load_cached_view(&overlapping_view)
                        .await
                        .expect("the other view remains readable")
                        .write_readiness()
                        == CalendarWriteReadiness::AccountSyncRequired
                );
            }
        }
        let reconciled = workflow
            .load_cached_view(&overlapping_view)
            .await
            .expect("overlapping event view is reconciled")
            .events()[0]
            .clone();
        assert_eq!(reconciled.subject(), Some("Updated cross-week appointment"));
        assert!(workflow.load_versioned_event(&reconciled).await.is_ok());
    });
}

#[test]
fn event_update_reserves_the_whole_account_until_the_cache_commit_finishes() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let cache = FixtureCache::default();
        let workflow = std::sync::Arc::new(CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            cache,
        ));
        synchronize_empty_view(
            &workflow,
            &provider,
            &view,
            "event-update-reservation-initial-sync",
        )
        .await;
        let creation = creation(view.clone());
        let CalendarEventCreationOutcome::Cached(event) = workflow
            .create_event(&creation)
            .await
            .expect("synthetic event creation is confirmed")
        else {
            panic!("synthetic event is cached");
        };
        let editable = workflow
            .load_versioned_event(&event)
            .await
            .expect("synthetic timed event can be opened for editing");
        let draft = CalendarEventUpdateDraft::timed(
            "Updated planning".to_owned(),
            utc(2030, Month::January, 8) + time::Duration::hours(11),
            utc(2030, Month::January, 8) + time::Duration::hours(12),
        )
        .expect("synthetic edit draft is valid");
        let update = CalendarEventUpdate::new(editable, draft).expect("event values changed");
        let (started, release) = provider.hold_next_update();
        let updating_workflow = std::sync::Arc::clone(&workflow);
        let update_task = smol::spawn(async move { updating_workflow.update_event(&update).await });
        started
            .await
            .expect("synthetic provider started the held update");

        let another_week = view_for_week(14);
        assert_eq!(
            workflow.sync_view(&another_week).await,
            Err(CalendarWorkflowError::OperationInProgress)
        );
        release
            .send(())
            .expect("the synthetic update request is waiting for release");
        assert!(matches!(
            update_task.await,
            Ok(CalendarEventUpdateOutcome::Cached(_))
        ));
    });
}
