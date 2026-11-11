fn mismatched_creation_event(creation: &CalendarEventCreation) -> CalendarOccurrence {
    let CalendarOccurrenceTime::Timed { start, end } = creation.draft().time() else {
        panic!("synthetic creation fixture uses a timed draft");
    };
    let mut patch = CalendarOccurrencePatch::new();
    patch.set_subject(Some("Different synthetic subject".to_owned()));
    let mut time = CalendarOccurrenceTimePatch::new();
    time.set_start(*start);
    time.set_end(*end);
    time.set_is_all_day(false);
    patch.set_time(time);
    patch
        .into_new(
            creation.view().clone(),
            EventId::try_from("synthetic-mismatched-event".to_owned())
                .expect("synthetic event ID is valid"),
        )
        .expect("synthetic mismatched event has a valid time")
}

#[test]
fn event_creation_waits_for_a_successful_view_sync() {
    smol::block_on(async {
        let view = view();
        let creation = creation(view.clone());
        let provider = FixtureProvider::default();
        let workflow = CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        );

        assert_eq!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("the unsynchronized view remains readable")
                .write_readiness(),
            CalendarWriteReadiness::ViewSyncRequired
        );
        assert_eq!(
            workflow.create_event(&creation).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        let snapshot = workflow
            .load_cached_view(&view)
            .await
            .expect("the unsynchronized view remains readable");
        assert!(snapshot.pending_creation().is_none());
        assert!(
            provider
                .creations
                .lock()
                .expect("synthetic creation log mutex is not poisoned")
                .is_empty()
        );

        synchronize_empty_view(&workflow, &provider, &view, "creation-initial-sync").await;
        assert_eq!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("the synchronized view remains readable")
                .write_readiness(),
            CalendarWriteReadiness::Ready
        );
        assert!(matches!(
            workflow.create_event(&creation).await,
            Ok(CalendarEventCreationOutcome::Cached(_))
        ));
    });
}

#[test]
fn event_creation_reuses_its_pending_id_after_an_uncertain_provider_failure() {
    smol::block_on(async {
        let view = view();
        let creation = creation(view.clone());
        let provider = FixtureProvider::default();
        provider.fail_next_creation(CalendarOperationError::ServiceUnavailable);
        let cache = FixtureCache::default();
        let workflow = CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache);
        synchronize_empty_view(&workflow, &provider, &view, "create-retry-initial-sync").await;

        assert_eq!(
            workflow.create_event(&creation).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::ServiceUnavailable
            ))
        );
        let pending = workflow
            .load_cached_view(&view)
            .await
            .expect("synthetic pending operation can be read");
        assert_eq!(
            pending.pending_creation().map(|item| item.creation().id()),
            Some(creation.id())
        );
        assert!(pending.events().is_empty());

        let outcome = workflow
            .create_event(&creation)
            .await
            .expect("same operation confirms on retry");
        assert!(matches!(outcome, CalendarEventCreationOutcome::Cached(_)));
        assert_eq!(
            *provider
                .creations
                .lock()
                .expect("synthetic creation log mutex is not poisoned"),
            vec![creation.id(), creation.id()]
        );
        let saved = workflow
            .load_cached_view(&view)
            .await
            .expect("confirmed event is readable from the synthetic cache");
        assert!(saved.pending_creation().is_none());
        assert_eq!(saved.events().len(), 1);
        assert_eq!(
            saved.events()[0].subject(),
            Some("Synthetic planning meeting")
        );
    });
}

#[test]
fn event_creation_recovers_when_cache_commit_fails_after_graph_confirmation() {
    smol::block_on(async {
        let view = view();
        let creation = creation(view.clone());
        let provider = FixtureProvider::default();
        let cache = FixtureCache::default();
        cache.fail_creation_commit(CalendarCacheError::Unavailable);
        let workflow =
            CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache.clone());
        synchronize_empty_view(&workflow, &provider, &view, "create-cache-initial-sync").await;

        let first_outcome = workflow
            .create_event(&creation)
            .await
            .expect("Graph confirmation is reported even when local commit fails");
        assert!(matches!(
            first_outcome,
            CalendarEventCreationOutcome::ConfirmedButNotCached {
                cache_error: CalendarCacheError::Unavailable,
                ..
            }
        ));
        let pending = workflow
            .load_cached_view(&view)
            .await
            .expect("the confirmed operation remains available for recovery");
        assert_eq!(
            pending
                .pending_creation()
                .map(|pending| pending.creation().id()),
            Some(creation.id())
        );
        assert!(pending.events().is_empty());

        let retry_outcome = workflow
            .create_event(&creation)
            .await
            .expect("retry confirms and saves the same operation");
        assert!(matches!(
            retry_outcome,
            CalendarEventCreationOutcome::Cached(_)
        ));
        assert_eq!(
            *provider
                .creations
                .lock()
                .expect("synthetic creation log mutex is not poisoned"),
            vec![creation.id(), creation.id()]
        );
        let saved = workflow
            .load_cached_view(&view)
            .await
            .expect("saved event is readable from the synthetic cache");
        assert!(saved.pending_creation().is_none());
        assert_eq!(saved.events().len(), 1);
    });
}

#[test]
fn event_creation_rejects_an_unrelated_pending_operation_for_the_account() {
    smol::block_on(async {
        let view = view();
        let pending_creation = creation(view.clone());
        let new_creation = creation(view);
        let provider = FixtureProvider::default();
        let cache = FixtureCache::default();
        let workflow =
            CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache.clone());
        synchronize_empty_view(
            &workflow,
            &provider,
            pending_creation.view(),
            "create-pending-initial-sync",
        )
        .await;
        cache
            .stage_event_creation(&pending_creation)
            .await
            .expect("the synthetic pending operation is staged");

        assert_eq!(
            workflow.create_event(&new_creation).await,
            Err(CalendarWorkflowError::Cache(
                CalendarCacheError::CreationAlreadyPending
            ))
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

#[test]
fn definitive_event_creation_rejection_clears_its_pending_operation() {
    smol::block_on(async {
        let view = view();
        let creation = creation(view.clone());
        let provider = FixtureProvider::default();
        provider.fail_next_creation(CalendarOperationError::EventCreationRejected);
        let cache = FixtureCache::default();
        let workflow = CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache);
        synchronize_empty_view(&workflow, &provider, &view, "create-rejected-initial-sync").await;

        assert_eq!(
            workflow.create_event(&creation).await,
            Ok(CalendarEventCreationOutcome::Rejected)
        );
        assert!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("the rejected event leaves no pending marker")
                .pending_creation()
                .is_none()
        );
        assert_eq!(
            *provider
                .creations
                .lock()
                .expect("synthetic creation log mutex is not poisoned"),
            vec![creation.id()]
        );
    });
}

#[test]
fn rejected_event_cleanup_can_resume_without_reposting_to_graph() {
    smol::block_on(async {
        let view = view();
        let creation = creation(view.clone());
        let provider = FixtureProvider::default();
        provider.fail_next_creation(CalendarOperationError::EventCreationRejected);
        let cache = FixtureCache::default();
        cache.fail_rejection_cleanup(CalendarCacheError::Unavailable);
        let workflow = CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache);
        synchronize_empty_view(&workflow, &provider, &view, "create-cleanup-initial-sync").await;

        assert_eq!(
            workflow.create_event(&creation).await,
            Ok(CalendarEventCreationOutcome::RejectedButPending {
                cache_error: CalendarCacheError::Unavailable,
            })
        );
        let pending = workflow
            .load_cached_view(&view)
            .await
            .expect("the definitive rejection remains visible for local cleanup");
        assert_eq!(
            pending.pending_creation(),
            Some(&PendingCalendarEventCreation::Rejected(creation.clone()))
        );

        workflow
            .discard_rejected_event_creation(&creation)
            .await
            .expect("local cleanup retries without another provider request");
        assert!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("the rejected marker is removed")
                .pending_creation()
                .is_none()
        );
        assert_eq!(
            *provider
                .creations
                .lock()
                .expect("synthetic creation log mutex is not poisoned"),
            vec![creation.id()]
        );
    });
}

#[test]
fn rejected_event_can_be_resolved_after_persisting_the_rejection_recovers() {
    smol::block_on(async {
        let view = view();
        let creation = creation(view.clone());
        let provider = FixtureProvider::default();
        provider.fail_next_creation(CalendarOperationError::EventCreationRejected);
        let cache = FixtureCache::default();
        cache.fail_rejection_mark(CalendarCacheError::Unavailable);
        let workflow = CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache);
        synchronize_empty_view(
            &workflow,
            &provider,
            &view,
            "create-rejection-mark-initial-sync",
        )
        .await;

        assert_eq!(
            workflow.create_event(&creation).await,
            Ok(CalendarEventCreationOutcome::RejectedButPending {
                cache_error: CalendarCacheError::Unavailable,
            })
        );
        assert_eq!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("the unresolved draft remains available")
                .pending_creation(),
            Some(&PendingCalendarEventCreation::Unresolved(creation.clone()))
        );

        workflow
            .discard_rejected_event_creation(&creation)
            .await
            .expect("the stored rejection can be recorded and cleared locally");
        assert!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("the rejected operation is cleared")
                .pending_creation()
                .is_none()
        );
        assert_eq!(
            *provider
                .creations
                .lock()
                .expect("synthetic creation log mutex is not poisoned"),
            vec![creation.id()]
        );
    });
}

#[test]
fn event_creation_keeps_the_pending_operation_when_graph_confirms_another_event() {
    smol::block_on(async {
        let view = view();
        let creation = creation(view.clone());
        let provider = FixtureProvider::default();
        provider.return_creation(mismatched_creation_event(&creation));
        let workflow = CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        );
        synchronize_empty_view(&workflow, &provider, &view, "create-mismatch-initial-sync").await;

        assert_eq!(
            workflow.create_event(&creation).await,
            Err(CalendarWorkflowError::InvalidConfirmation)
        );
        let snapshot = workflow
            .load_cached_view(&view)
            .await
            .expect("unconfirmed operation remains visible for recovery");
        assert_eq!(
            snapshot.pending_creation().map(|item| item.creation().id()),
            Some(creation.id())
        );
        assert!(snapshot.events().is_empty());
    });
}

#[test]
fn event_creation_reserves_the_account_while_another_view_sync_is_in_flight() {
    smol::block_on(async {
        let view = view();
        let another_view = view_for_week(6);
        let provider = FixtureProvider::default();
        let workflow = std::sync::Arc::new(CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        ));
        synchronize_empty_view(&workflow, &provider, &view, "create-sync-race-source").await;
        synchronize_empty_view(
            &workflow,
            &provider,
            &another_view,
            "create-sync-race-target",
        )
        .await;
        provider.push_page(Ok(response(
            Vec::new(),
            CalendarDeltaProgress::Complete(link("create-sync-race-followup")),
        )));

        let (started, release) = provider.hold_next_delta();
        let syncing_workflow = std::sync::Arc::clone(&workflow);
        let syncing_view = view.clone();
        let sync = smol::spawn(async move { syncing_workflow.sync_view(&syncing_view).await });
        started
            .await
            .expect("the synthetic delta request has started");

        assert_eq!(
            workflow.create_event(&creation(another_view)).await,
            Err(CalendarWorkflowError::OperationInProgress)
        );
        assert!(
            provider
                .creations
                .lock()
                .expect("synthetic creation log mutex is not poisoned")
                .is_empty()
        );

        release
            .send(())
            .expect("the synthetic delta request is waiting for release");
        assert!(sync.await.is_ok());
    });
}
