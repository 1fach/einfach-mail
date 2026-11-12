#[test]
fn confirmed_event_deletion_removes_the_cached_event() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let workflow = CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        );
        synchronize_empty_view(&workflow, &provider, &view, "delete-initial-sync").await;
        let CalendarEventCreationOutcome::Cached(event) = workflow
            .create_event(&creation(view.clone()))
            .await
            .expect("synthetic event creation is confirmed")
        else {
            panic!("synthetic event is cached");
        };
        let versioned_event = workflow
            .load_versioned_event(&event)
            .await
            .expect("synthetic event has a provider version");

        assert_eq!(
            workflow
                .delete_event(&versioned_event)
                .await
                .expect("provider deletion is confirmed"),
            CalendarEventDeletionOutcome::Deleted
        );
        let snapshot = workflow
            .load_cached_view(&view)
            .await
            .expect("deleted view remains readable");
        assert!(snapshot.events().is_empty());
        assert_eq!(snapshot.write_readiness(), CalendarWriteReadiness::Ready);
        let deleted_events = provider
            .deletion_events
            .lock()
            .expect("synthetic deletion event mutex is not poisoned")
            .clone();
        assert_eq!(deleted_events.len(), 1);
        assert_eq!(deleted_events[0].occurrence().id(), event.id());
        assert_eq!(deleted_events[0].version().as_str(), "synthetic-version-1");
        assert_eq!(
            *provider
                .deletion_calls
                .lock()
                .expect("synthetic deletion counter mutex is not poisoned"),
            1
        );
    });
}

#[test]
fn uncertain_event_deletion_requires_sync_and_is_not_replayed() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let workflow = CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        );
        synchronize_empty_view(&workflow, &provider, &view, "delete-unknown-initial-sync").await;
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
        provider.fail_next_deletion(CalendarOperationError::DeleteOutcomeUnknown);

        assert_eq!(
            workflow.delete_event(&editable).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::DeleteOutcomeUnknown
            ))
        );
        assert!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("cached view remains readable")
                .write_readiness()
                == CalendarWriteReadiness::AccountSyncRequired
        );
        assert_eq!(
            workflow.delete_event(&editable).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert_eq!(
            *provider
                .deletion_calls
                .lock()
                .expect("synthetic deletion counter mutex is not poisoned"),
            1,
            "an uncertain deletion must not be replayed before sync"
        );

        provider.push_page(Ok(response(
            vec![CalendarDeltaChange::Removed {
                id: event.id().clone(),
            }],
            CalendarDeltaProgress::Complete(link("after-unknown-delete")),
        )));
        workflow
            .sync_view(&view)
            .await
            .expect("a full sync reconciles the deletion outcome");
        let snapshot = workflow
            .load_cached_view(&view)
            .await
            .expect("reconciled view remains readable");
        assert!(snapshot.events().is_empty());
        assert_eq!(snapshot.write_readiness(), CalendarWriteReadiness::Ready);
    });
}

#[test]
fn uncertain_event_deletion_requires_sync_for_each_previously_synced_account_view() {
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
            (&source_view, "delete-cross-week-source-initial"),
            (&overlapping_view, "delete-cross-week-overlap-initial"),
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
        let source_editable = workflow
            .load_versioned_event(&source_event)
            .await
            .expect("synchronized event can be opened for deletion");
        let overlapping_editable = workflow
            .load_versioned_event(&overlapping_event)
            .await
            .expect("overlapping event can be opened for deletion");
        provider.fail_next_deletion(CalendarOperationError::DeleteOutcomeUnknown);

        assert_eq!(
            workflow.delete_event(&source_editable).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::DeleteOutcomeUnknown
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
            workflow.delete_event(&overlapping_editable).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );

        provider.push_page(Ok(response(
            vec![CalendarDeltaChange::Removed { id: event_id() }],
            CalendarDeltaProgress::Complete(link("delete-cross-week-source-recovered")),
        )));
        workflow
            .sync_view(&source_view)
            .await
            .expect("source view completes its recovery sync");
        assert!(
            workflow
                .load_cached_view(&source_view)
                .await
                .expect("reconciled source view is readable")
                .write_readiness()
                == CalendarWriteReadiness::AccountSyncRequired,
            "the source view remains gated while another account view needs recovery"
        );
        assert_eq!(
            workflow.delete_event(&source_editable).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired),
            "one view's recovery sync must not unlock writes while another account view remains stale"
        );
        assert!(
            workflow
                .load_cached_view(&overlapping_view)
                .await
                .expect("overlapping view remains readable")
                .write_readiness()
                == CalendarWriteReadiness::AccountSyncRequired
        );
        assert_eq!(
            workflow.delete_event(&overlapping_editable).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );

        provider.push_page(Ok(response(
            vec![CalendarDeltaChange::Removed { id: event_id() }],
            CalendarDeltaProgress::Complete(link("delete-cross-week-overlap-recovered")),
        )));
        workflow
            .sync_view(&overlapping_view)
            .await
            .expect("overlapping view completes its recovery sync");
        for view in [&source_view, &overlapping_view] {
            assert_eq!(
                workflow
                    .load_cached_view(view)
                    .await
                    .expect("reconciled event view is readable")
                    .write_readiness(),
                CalendarWriteReadiness::Ready
            );
        }
        assert_eq!(
            *provider
                .deletion_calls
                .lock()
                .expect("synthetic deletion counter mutex is not poisoned"),
            1,
            "an uncertain deletion is not replayed before all views sync"
        );
    });
}

#[test]
fn changed_event_deletion_requires_sync_before_another_write() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let workflow = CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        );
        synchronize_empty_view(&workflow, &provider, &view, "delete-changed-initial-sync").await;
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
        provider.fail_next_deletion(CalendarOperationError::EventChanged);

        assert_eq!(
            workflow.delete_event(&editable).await,
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
            workflow.delete_event(&editable).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert_eq!(
            *provider
                .deletion_calls
                .lock()
                .expect("synthetic deletion counter mutex is not poisoned"),
            1,
            "a changed event is not deleted again before sync"
        );
    });
}

#[test]
fn confirmed_event_deletion_with_a_cache_failure_requires_sync() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let cache = FixtureCache::default();
        cache.fail_deletion_commit(CalendarCacheError::Unavailable);
        let workflow = CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache);
        synchronize_empty_view(&workflow, &provider, &view, "delete-cache-initial-sync").await;
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

        assert_eq!(
            workflow.delete_event(&editable).await,
            Ok(CalendarEventDeletionOutcome::DeletedButNotCached {
                cache_error: CalendarCacheError::Unavailable,
            })
        );
        assert!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("cached view remains readable")
                .write_readiness()
                == CalendarWriteReadiness::AccountSyncRequired
        );
        assert_eq!(
            workflow.delete_event(&editable).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert_eq!(
            *provider
                .deletion_calls
                .lock()
                .expect("synthetic deletion counter mutex is not poisoned"),
            1,
            "a confirmed remote deletion is not repeated after a local failure"
        );
        provider.push_page(Ok(response(
            vec![CalendarDeltaChange::Removed {
                id: event.id().clone(),
            }],
            CalendarDeltaProgress::Complete(link("after-delete-cache-failure")),
        )));
        workflow
            .sync_view(&view)
            .await
            .expect("a full sync repairs the cache and clears the gate");
        let snapshot = workflow
            .load_cached_view(&view)
            .await
            .expect("repaired view remains readable");
        assert!(snapshot.events().is_empty());
        assert_eq!(snapshot.write_readiness(), CalendarWriteReadiness::Ready);
    });
}

#[test]
fn event_deletion_reserves_the_account_until_the_provider_returns() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let workflow = std::sync::Arc::new(CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        ));
        synchronize_empty_view(
            &workflow,
            &provider,
            &view,
            "delete-reservation-initial-sync",
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
        let second_editable = editable.clone();
        let (started, release) = provider.hold_next_deletion();
        let deleting_workflow = std::sync::Arc::clone(&workflow);
        let deletion = smol::spawn(async move { deleting_workflow.delete_event(&editable).await });
        started
            .await
            .expect("synthetic deletion request has started");

        assert_eq!(
            workflow.delete_event(&second_editable).await,
            Err(CalendarWorkflowError::OperationInProgress)
        );
        release
            .send(())
            .expect("synthetic provider is waiting for deletion release");
        assert_eq!(deletion.await, Ok(CalendarEventDeletionOutcome::Deleted));
        assert_eq!(
            *provider
                .deletion_calls
                .lock()
                .expect("synthetic deletion counter mutex is not poisoned"),
            1
        );
    });
}

#[test]
fn canceled_event_deletion_requires_sync_before_another_write() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let cache = FixtureCache::default();
        let workflows = workflows_sharing_account_state(&provider, &cache);
        synchronize_empty_view(
            &workflows.first,
            &provider,
            &view,
            "cancel-delete-initial-sync",
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
        let retry = editable.clone();
        let (started, _release) = provider.hold_next_deletion();
        let deleting_workflow = std::sync::Arc::clone(&workflows.first);
        let deletion = smol::spawn(async move { deleting_workflow.delete_event(&editable).await });
        started
            .await
            .expect("synthetic deletion request has started");

        assert!(deletion.cancel().await.is_none());
        assert_eq!(
            workflows.sibling.delete_event(&retry).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
    });
}

#[test]
fn reopened_workflow_reconciles_a_committed_delete_after_response_timeout() {
    smol::block_on(async {
        let view = view();
        let other_view = view_for_week(14);
        let provider = FixtureProvider::default();
        let cache = FixtureCache::default();
        let workflow =
            CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache.clone());
        synchronize_empty_view(&workflow, &provider, &view, "restart-delete-source").await;
        synchronize_empty_view(&workflow, &provider, &other_view, "restart-delete-other").await;
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
        provider.fail_next_deletion(CalendarOperationError::DeleteOutcomeUnknown);

        assert_eq!(
            workflow.delete_event(&editable).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::DeleteOutcomeUnknown
            ))
        );

        let restarted = CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache);
        provider.push_page(Ok(response(
            vec![CalendarDeltaChange::Removed {
                id: event.id().clone(),
            }],
            CalendarDeltaProgress::Complete(link("restart-delete-source-reconciled")),
        )));
        restarted
            .sync_view(&view)
            .await
            .expect("the source view completes its post-restart sync");
        let source_snapshot = restarted
            .load_cached_view(&view)
            .await
            .expect("the source view remains readable");
        assert_eq!(
            source_snapshot.write_readiness(),
            CalendarWriteReadiness::AccountSyncRequired
        );
        assert!(source_snapshot.events().is_empty());
        assert_eq!(
            restarted.create_event(&creation(view.clone())).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );

        provider.push_page(Ok(response(
            Vec::new(),
            CalendarDeltaProgress::Complete(link("restart-delete-other-reconciled")),
        )));
        restarted
            .sync_view(&other_view)
            .await
            .expect("the other previously synced view completes its post-restart sync");
        let reconciled_snapshot = restarted
            .load_cached_view(&view)
            .await
            .expect("all account views are reconciled");
        assert_eq!(
            reconciled_snapshot.write_readiness(),
            CalendarWriteReadiness::Ready
        );
        assert!(reconciled_snapshot.events().is_empty());
        assert!(matches!(
            restarted.create_event(&creation(view)).await,
            Ok(CalendarEventCreationOutcome::Cached(_))
        ));
        assert_eq!(
            *provider
                .deletion_calls
                .lock()
                .expect("synthetic deletion counter mutex is not poisoned"),
            1,
            "a committed deletion with a lost response is never replayed"
        );
    });
}
