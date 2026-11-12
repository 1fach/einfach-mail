use crate::test_support::{
    FixtureCache, FixtureProvider, creation, synchronize_empty_view, update_request, view,
    workflow_account_id, workflows_sharing_account_state,
};
use crate::{
    CalendarCache, CalendarCacheError, CalendarEventCreationOutcome, CalendarOperationError,
    CalendarWorkflow, CalendarWorkflowError, CalendarWriteReadiness,
};

#[test]
fn uncertain_update_and_delete_recovery_blocks_sibling_workflows() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let cache = FixtureCache::default();
        let workflows = workflows_sharing_account_state(&provider, &cache);
        synchronize_empty_view(
            &workflows.first,
            &provider,
            &view,
            "shared-recovery-initial-sync",
        )
        .await;
        let CalendarEventCreationOutcome::Cached(event) = workflows
            .first
            .create_event(&creation(view.clone()))
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
        let sibling_update = update_request(editable.clone());
        provider.fail_next_update(CalendarOperationError::UpdateOutcomeUnknown);

        assert_eq!(
            workflows.first.update_event(&update).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::UpdateOutcomeUnknown
            ))
        );
        assert_eq!(
            workflows.sibling.update_event(&sibling_update).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );

        synchronize_empty_view(
            &workflows.sibling,
            &provider,
            &view,
            "shared-recovery-update-sync",
        )
        .await;
        let editable = workflows
            .sibling
            .load_versioned_event(&event)
            .await
            .expect("successful sync restores calendar writes");
        let sibling_editable = editable.clone();
        provider.fail_next_deletion(CalendarOperationError::DeleteOutcomeUnknown);

        assert_eq!(
            workflows.first.delete_event(&editable).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::DeleteOutcomeUnknown
            ))
        );
        assert_eq!(
            workflows.sibling.delete_event(&sibling_editable).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
    });
}

#[test]
fn definitive_event_write_rejections_clear_persisted_recovery_markers() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let cache = FixtureCache::default();
        let workflow =
            CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache.clone());
        synchronize_empty_view(
            &workflow,
            &provider,
            &view,
            "definitive-write-rejection-initial-sync",
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
            .expect("synthetic event is editable");
        let update = update_request(editable);
        provider.fail_next_update(CalendarOperationError::AccessDenied);

        assert_eq!(
            workflow.update_event(&update).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::AccessDenied
            ))
        );
        assert!(
            !cache
                .account_has_calendar_write_sync_required(view.account_id())
                .await
                .expect("definitive update rejection clears its recovery marker")
        );
        assert_eq!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("cached view remains readable")
                .write_readiness(),
            CalendarWriteReadiness::Ready
        );

        let editable = workflow
            .load_versioned_event(&event)
            .await
            .expect("an access rejection keeps the event editable");
        let update = update_request(editable);
        provider.fail_next_update(CalendarOperationError::EventUpdateRejected);
        assert_eq!(
            workflow.update_event(&update).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::EventUpdateRejected
            ))
        );
        assert!(
            !cache
                .account_has_calendar_write_sync_required(view.account_id())
                .await
                .expect("a known update rejection clears its recovery marker")
        );
        assert_eq!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("cached view stays ready after known update rejection")
                .write_readiness(),
            CalendarWriteReadiness::Ready
        );

        let editable = workflow
            .load_versioned_event(&event)
            .await
            .expect("a known update rejection keeps the event editable");
        provider.fail_next_deletion(CalendarOperationError::EventDeletionRejected);
        assert_eq!(
            workflow.delete_event(&editable).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::EventDeletionRejected
            ))
        );
        assert!(
            !cache
                .account_has_calendar_write_sync_required(view.account_id())
                .await
                .expect("definitive deletion rejection clears its recovery marker")
        );
        assert_eq!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("cached view stays ready after definitive rejection")
                .write_readiness(),
            CalendarWriteReadiness::Ready
        );
    });
}

#[test]
fn failed_recovery_marker_cleanup_keeps_calendar_writes_blocked() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let cache = FixtureCache::default();
        let workflow =
            CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache.clone());
        synchronize_empty_view(&workflow, &provider, &view, "cleanup-failure-initial-sync").await;
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
        cache.fail_write_recovery_cleanup(CalendarCacheError::Unavailable);
        provider.fail_next_deletion(CalendarOperationError::EventDeletionRejected);

        assert_eq!(
            workflow.delete_event(&editable).await,
            Err(CalendarWorkflowError::Cache(
                CalendarCacheError::Unavailable
            ))
        );
        assert!(
            cache
                .account_has_calendar_write_sync_required(view.account_id())
                .await
                .expect("failed cleanup leaves the persistent recovery marker")
        );
        assert_eq!(
            workflow.create_event(&creation(view.clone())).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert_eq!(
            *provider
                .deletion_calls
                .lock()
                .expect("synthetic deletion counter mutex is not poisoned"),
            1,
            "the rejected deletion is attempted only once"
        );
        assert_eq!(
            provider
                .creations
                .lock()
                .expect("synthetic creation log mutex is not poisoned")
                .len(),
            1,
            "a new event write does not reach the provider"
        );
    });
}
