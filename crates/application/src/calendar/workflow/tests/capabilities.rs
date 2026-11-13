use einfach_mail_domain::{CalendarCapabilities, CalendarDeltaChange, CalendarDeltaProgress};

use crate::test_support::{
    FixtureCache, FixtureProvider, creation, cross_week_timed_event_patch, event_id, link,
    response, synchronize_empty_view, update_request, view, workflow_account_id,
};
use crate::{
    CalendarCache, CalendarEventCreationStage, CalendarOperationError, CalendarSession,
    CalendarWorkflow, CalendarWorkflowError, CalendarWriteReadiness,
};

#[test]
fn unsupported_writes_leave_recovery_state_and_provider_unchanged() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default()
            .with_calendar_capabilities(CalendarCapabilities::view_only());
        let cache = FixtureCache::default();
        let workflow =
            CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache.clone());
        let calendar: &dyn CalendarSession = &workflow;
        provider.push_page(Ok(response(
            vec![CalendarDeltaChange::Upsert {
                id: event_id(),
                patch: cross_week_timed_event_patch(),
            }],
            CalendarDeltaProgress::Complete(link("view-only-calendar-capabilities")),
        )));
        calendar
            .synchronize_view(&view)
            .await
            .expect("the view-only provider can synchronize its calendar");

        let event = calendar
            .load_cached_view(&view)
            .await
            .expect("the synchronized view remains readable")
            .events()[0]
            .clone();
        let editable = calendar
            .load_versioned_event(&event)
            .await
            .expect("the provider can read a versioned event");
        let update = update_request(editable.clone());
        let creation = creation(view.clone());

        assert_eq!(
            calendar.create_event(&creation).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::UnsupportedOperation
            ))
        );
        assert_eq!(
            calendar.update_event(&update).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::UnsupportedOperation
            ))
        );
        assert_eq!(
            calendar.delete_event(&editable).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::UnsupportedOperation
            ))
        );

        let snapshot = calendar
            .load_cached_view(&view)
            .await
            .expect("the calendar remains readable after rejected writes");
        assert_eq!(snapshot.write_readiness(), CalendarWriteReadiness::Ready);
        assert!(snapshot.pending_creation().is_none());
        assert!(
            provider
                .creations
                .lock()
                .expect("synthetic creation log mutex is not poisoned")
                .is_empty()
        );
        assert!(
            provider
                .update_calls
                .lock()
                .expect("synthetic update log mutex is not poisoned")
                .is_empty()
        );
        assert_eq!(
            *provider
                .deletion_calls
                .lock()
                .expect("synthetic deletion counter mutex is not poisoned"),
            0
        );
    });
}

#[test]
fn view_only_capabilities_leave_creation_status_recovery_available() {
    smol::block_on(async {
        let view = view();
        let creation = creation(view.clone());
        let provider = FixtureProvider::default()
            .with_calendar_capabilities(CalendarCapabilities::view_only());
        let cache = FixtureCache::default();
        let workflow =
            CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache.clone());
        synchronize_empty_view(&workflow, &provider, &view, "view-only-create-recovery").await;
        assert_eq!(
            cache.stage_event_creation(&creation).await,
            Ok(CalendarEventCreationStage::NewlyStaged)
        );

        assert!(matches!(
            workflow.recover_event_creation(&creation).await,
            Ok(crate::CalendarEventCreationOutcome::Cached(_))
        ));
        assert!(
            provider
                .creations
                .lock()
                .expect("synthetic creation log mutex is not poisoned")
                .is_empty()
        );
        assert_eq!(
            *provider
                .recoveries
                .lock()
                .expect("synthetic recovery log mutex is not poisoned"),
            vec![creation.id()]
        );
    });
}

#[test]
fn view_only_capabilities_leave_rejected_creation_cleanup_available() {
    smol::block_on(async {
        let view = view();
        let creation = creation(view.clone());
        let provider = FixtureProvider::default()
            .with_calendar_capabilities(CalendarCapabilities::view_only());
        let cache = FixtureCache::default();
        let workflow =
            CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache.clone());
        synchronize_empty_view(&workflow, &provider, &view, "view-only-create-cleanup").await;
        assert_eq!(
            cache.stage_event_creation(&creation).await,
            Ok(CalendarEventCreationStage::NewlyStaged)
        );
        cache
            .mark_event_creation_rejected(&creation)
            .await
            .expect("the synthetic cache can record the provider's rejection");

        workflow
            .discard_rejected_event_creation(&creation)
            .await
            .expect("local rejected-operation cleanup remains available");

        assert!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("the calendar remains readable after local cleanup")
                .pending_creation()
                .is_none()
        );
        assert!(
            provider
                .creations
                .lock()
                .expect("synthetic creation log mutex is not poisoned")
                .is_empty()
        );
    });
}
