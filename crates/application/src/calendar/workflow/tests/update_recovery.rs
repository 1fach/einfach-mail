#[test]
fn event_update_confirms_and_replaces_the_cached_occurrence() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let cache = FixtureCache::default();
        let workflow = CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache);
        synchronize_empty_view(&workflow, &provider, &view, "event-update-initial-sync").await;
        let creation = creation(view.clone());
        let CalendarEventCreationOutcome::Cached(event) = workflow
            .create_event(&creation)
            .await
            .expect("synthetic event creation is confirmed")
        else {
            panic!("synthetic event is cached");
        };
        let versioned_event = workflow
            .load_versioned_event(&event)
            .await
            .expect("synthetic timed event has a provider version");
        let draft = CalendarEventUpdateDraft::timed(
            "Updated planning".to_owned(),
            utc(2030, Month::January, 8) + time::Duration::hours(11),
            utc(2030, Month::January, 8) + time::Duration::hours(12),
        )
        .expect("synthetic edit draft is valid");
        let update = CalendarEventUpdate::new(versioned_event, draft).expect("event values changed");

        let outcome = workflow
            .update_event(&update)
            .await
            .expect("synthetic Graph update is confirmed");
        assert!(matches!(outcome, CalendarEventUpdateOutcome::Cached(_)));
        let captured_updates = provider
            .update_calls
            .lock()
            .expect("synthetic update log mutex is not poisoned")
            .clone();
        assert_eq!(captured_updates.len(), 1);
        assert_eq!(
            captured_updates[0].source().occurrence().id(),
            event.id()
        );
        assert_eq!(
            captured_updates[0].source().version().as_str(),
            "synthetic-version-1"
        );
        let snapshot = workflow
            .load_cached_view(&view)
            .await
            .expect("updated event remains cached");
        assert_eq!(snapshot.events().len(), 1);
        assert_eq!(snapshot.events()[0].subject(), Some("Updated planning"));
        assert_eq!(snapshot.events()[0].time(), update.draft().time());
    });
}

#[test]
fn event_update_reports_remote_confirmation_when_cache_commit_fails() {
    smol::block_on(async {
        let view = view();
        let provider = FixtureProvider::default();
        let cache = FixtureCache::default();
        cache.fail_update_commit(CalendarCacheError::Unavailable);
        let workflow = CalendarWorkflow::new(workflow_account_id(), provider.clone(), cache);
        synchronize_empty_view(
            &workflow,
            &provider,
            &view,
            "event-update-cache-initial-sync",
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

        let outcome = workflow
            .update_event(&update)
            .await
            .expect("a confirmed remote edit remains a successful outcome");
        assert!(matches!(
            outcome,
            CalendarEventUpdateOutcome::ConfirmedButNotCached {
                cache_error: CalendarCacheError::Unavailable,
                ..
            }
        ));
        assert!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("the failed cache commit leaves the view readable")
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
            provider
                .update_calls
                .lock()
                .expect("synthetic update log mutex is not poisoned")
                .len(),
            1,
            "a local cache failure must not repeat the remote update"
        );
        provider.push_page(Ok(response(
            Vec::new(),
            CalendarDeltaProgress::Complete(link("after-update-cache-failure")),
        )));
        workflow
            .sync_view(&view)
            .await
            .expect("a completed sync clears the update recovery gate");
        assert!(workflow.load_versioned_event(&event).await.is_ok());
    });
}

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
            .expect("definitive rejection keeps the event editable");
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
