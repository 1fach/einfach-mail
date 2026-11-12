use einfach_mail_domain::{
    CalendarDeltaChange, CalendarDeltaProgress, CalendarEventUpdate, CalendarEventUpdateDraft,
    CalendarOccurrencePatch,
};
use time::Month;

use crate::test_support::{
    FixtureCache, FixtureProvider, creation, link, response, synchronize_empty_view,
    update_request, utc, view, view_for_week, workflow_account_id, workflows_sharing_account_state,
};
use crate::{
    CalendarEventCreationOutcome, CalendarOperationError, CalendarWorkflow, CalendarWorkflowError,
    CalendarWriteReadiness,
};

#[test]
fn unknown_event_update_requires_successful_sync_before_another_write() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let workflow = CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        );
        synchronize_empty_view(
            &workflow,
            &provider,
            &view,
            "unknown-event-update-initial-sync",
        )
        .await;
        let CalendarEventCreationOutcome::Cached(event) = workflow
            .create_event(&creation(view.clone()))
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
        let followup_creation = creation(view.clone());
        provider.fail_next_update(CalendarOperationError::UpdateOutcomeUnknown);

        assert_eq!(
            workflow.update_event(&update).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::UpdateOutcomeUnknown
            ))
        );
        assert!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("cached event stays available")
                .write_readiness()
                == CalendarWriteReadiness::AccountSyncRequired
        );
        assert_eq!(
            workflow.load_versioned_event(&event).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert_eq!(
            workflow.update_event(&update).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert_eq!(
            workflow.create_event(&followup_creation).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("the unresolved update leaves the view readable")
                .pending_creation()
                .is_none()
        );
        assert_eq!(
            provider
                .creations
                .lock()
                .expect("synthetic creation log mutex is not poisoned")
                .len(),
            1,
            "a recovery-required account must not send another event creation"
        );
        assert!(matches!(
            workflow.sync_view(&view).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::ServiceUnavailable
            ))
        ));
        assert_eq!(
            workflow.update_event(&update).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        provider.push_page(Ok(response(
            Vec::new(),
            CalendarDeltaProgress::Complete(link("after-unknown-update")),
        )));
        workflow
            .sync_view(&view)
            .await
            .expect("a complete sync clears the recovery gate");
        assert!(workflow.load_versioned_event(&event).await.is_ok());
        assert!(matches!(
            workflow.create_event(&followup_creation).await,
            Ok(CalendarEventCreationOutcome::Cached(_))
        ));
        assert_eq!(
            provider
                .update_calls
                .lock()
                .expect("synthetic update log mutex is not poisoned")
                .len(),
            1,
            "uncertain updates are not replayed after sync"
        );
    });
}

#[test]
fn reopened_workflow_requires_sync_for_every_previously_synced_view_after_unknown_update() {
    smol::block_on(async {
        let source_view = view();
        let other_view = view_for_week(14);
        let provider = FixtureProvider::default();
        let cache = FixtureCache::default();
        let workflow =
            CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache.clone());
        synchronize_empty_view(
            &workflow,
            &provider,
            &source_view,
            "restart-update-source-initial",
        )
        .await;
        synchronize_empty_view(
            &workflow,
            &provider,
            &other_view,
            "restart-update-other-initial",
        )
        .await;
        let CalendarEventCreationOutcome::Cached(event) = workflow
            .create_event(&creation(source_view.clone()))
            .await
            .expect("synthetic event creation is confirmed")
        else {
            panic!("synthetic event is cached");
        };
        let editable = workflow
            .load_versioned_event(&event)
            .await
            .expect("synthetic timed event is editable");
        let update = update_request(editable);
        provider.fail_next_update(CalendarOperationError::UpdateOutcomeUnknown);

        assert_eq!(
            workflow.update_event(&update).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::UpdateOutcomeUnknown
            ))
        );

        let restarted = CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache);
        let mut patch = CalendarOccurrencePatch::new();
        patch.set_subject(Some(update.draft().subject().to_owned()));
        provider.push_page(Ok(response(
            vec![CalendarDeltaChange::Upsert {
                id: event.id().clone(),
                patch,
            }],
            CalendarDeltaProgress::Complete(link("restart-update-source-reconciled")),
        )));
        restarted
            .sync_view(&source_view)
            .await
            .expect("the source view completes its post-restart sync");
        assert_eq!(
            restarted
                .load_cached_view(&source_view)
                .await
                .expect("the source view remains readable")
                .write_readiness(),
            CalendarWriteReadiness::AccountSyncRequired
        );
        assert_eq!(
            restarted.create_event(&creation(source_view.clone())).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );

        provider.push_page(Ok(response(
            Vec::new(),
            CalendarDeltaProgress::Complete(link("restart-update-other-reconciled")),
        )));
        restarted
            .sync_view(&other_view)
            .await
            .expect("the other previously synced view completes its post-restart sync");
        let snapshot = restarted
            .load_cached_view(&source_view)
            .await
            .expect("all account views are reconciled");
        assert_eq!(snapshot.write_readiness(), CalendarWriteReadiness::Ready);
        assert_eq!(
            snapshot.events()[0].subject(),
            Some(update.draft().subject())
        );
        assert_eq!(
            provider
                .update_calls
                .lock()
                .expect("synthetic update log mutex is not poisoned")
                .len(),
            1,
            "an uncertain update is not replayed after restart and sync"
        );
    });
}

#[test]
fn changed_event_update_requires_sync_before_another_write() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let workflow = CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        );
        synchronize_empty_view(&workflow, &provider, &view, "update-changed-initial-sync").await;
        let CalendarEventCreationOutcome::Cached(event) = workflow
            .create_event(&creation(view.clone()))
            .await
            .expect("synthetic event creation is confirmed")
        else {
            panic!("synthetic event is cached");
        };
        let editable = workflow
            .load_versioned_event(&event)
            .await
            .expect("synthetic event is editable");
        let draft = CalendarEventUpdateDraft::timed(
            "Updated planning".to_owned(),
            utc(2030, Month::January, 8) + time::Duration::hours(11),
            utc(2030, Month::January, 8) + time::Duration::hours(12),
        )
        .expect("synthetic edit draft is valid");
        let update = CalendarEventUpdate::new(editable, draft).expect("event values changed");
        provider.fail_next_update(CalendarOperationError::EventChanged);

        assert_eq!(
            workflow.update_event(&update).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::EventChanged
            ))
        );
        assert!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("cached event remains available")
                .write_readiness()
                == CalendarWriteReadiness::AccountSyncRequired
        );
        assert_eq!(
            workflow.update_event(&update).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert_eq!(
            provider
                .update_calls
                .lock()
                .expect("synthetic update log mutex is not poisoned")
                .len(),
            1,
            "a changed event is not updated again before sync"
        );
    });
}

#[test]
fn canceled_event_update_requires_sync_before_another_write() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let cache = FixtureCache::default();
        let workflows = workflows_sharing_account_state(&provider, &cache);
        synchronize_empty_view(
            &workflows.first,
            &provider,
            &view,
            "cancel-update-initial-sync",
        )
        .await;
        let CalendarEventCreationOutcome::Cached(event) = workflows
            .first
            .create_event(&creation(view))
            .await
            .expect("synthetic event creation is confirmed")
        else {
            panic!("synthetic event is cached");
        };
        let editable = workflows
            .first
            .load_versioned_event(&event)
            .await
            .expect("synthetic event is editable");
        let update = update_request(editable.clone());
        let retry = update_request(editable);
        let (started, _release) = provider.hold_next_update();
        let updating_workflow = std::sync::Arc::clone(&workflows.first);
        let update_task = smol::spawn(async move { updating_workflow.update_event(&update).await });
        started.await.expect("synthetic update request has started");

        assert!(update_task.cancel().await.is_none());
        assert_eq!(
            workflows.sibling.update_event(&retry).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
    });
}
