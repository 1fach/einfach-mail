#[test]
fn replacement_sync_keeps_the_old_snapshot_visible_until_final_commit() {
    smol::block_on(async {
        let view = view();
        let old_delta = link("synthetic-old-delta");
        let reset_link = link("synthetic-reset");
        let replacement_next = link("synthetic-replacement-next");
        let new_delta = link("synthetic-new-delta");
        let provider = FixtureProvider::with_pages([Ok(response(
            vec![CalendarDeltaChange::Upsert {
                id: event_id(),
                patch: all_day_patch(Some("Old snapshot"), None),
            }],
            CalendarDeltaProgress::Complete(old_delta.clone()),
        ))]);
        let request_log = Arc::clone(&provider.requests);
        let workflow = CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        );

        workflow
            .sync_view(&view)
            .await
            .expect("the initial snapshot commits");
        provider.push_page(Ok(CalendarDeltaResponse::Reset {
            reset_link: Some(reset_link.clone()),
            response_bytes: 96,
        }));
        provider.push_page(Ok(response(
            vec![CalendarDeltaChange::Upsert {
                id: replacement_event_id(),
                patch: all_day_patch(Some("Replacement snapshot"), None),
            }],
            CalendarDeltaProgress::Next(replacement_next.clone()),
        )));
        provider.push_page(Err(CalendarOperationError::ServiceUnavailable));

        assert_eq!(
            workflow.sync_view(&view).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::ServiceUnavailable
            ))
        );
        let interrupted = workflow
            .load_cached_view(&view)
            .await
            .expect("the previous complete snapshot remains readable");
        assert_eq!(interrupted.events().len(), 1);
        assert_eq!(interrupted.events()[0].subject(), Some("Old snapshot"));
        assert_eq!(interrupted.sync_state().delta_link(), Some(&old_delta));
        assert_eq!(
            interrupted.sync_state().next_link(),
            Some(&replacement_next)
        );
        assert!(interrupted.sync_state().replacement_active());

        provider.push_page(Ok(response(
            Vec::new(),
            CalendarDeltaProgress::Complete(new_delta.clone()),
        )));
        workflow
            .sync_view(&view)
            .await
            .expect("the replacement snapshot completes");
        let completed = workflow
            .load_cached_view(&view)
            .await
            .expect("the replacement snapshot is readable");
        assert_eq!(completed.events().len(), 1);
        assert_eq!(
            completed.events()[0].subject(),
            Some("Replacement snapshot")
        );
        assert_eq!(completed.sync_state().delta_link(), Some(&new_delta));
        assert!(!completed.sync_state().replacement_active());
        assert_eq!(
            *request_log
                .lock()
                .expect("synthetic request log mutex is not poisoned"),
            vec![
                CalendarDeltaRequest::Initial,
                CalendarDeltaRequest::Continue(old_delta),
                CalendarDeltaRequest::Continue(reset_link),
                CalendarDeltaRequest::Continue(replacement_next.clone()),
                CalendarDeltaRequest::Continue(replacement_next),
            ]
        );
    });
}

#[test]
fn a_second_reset_is_persisted_and_stops_the_current_attempt() {
    smol::block_on(async {
        let view = view();
        let old_delta = link("synthetic-old-delta");
        let reset_link = link("synthetic-reset");
        let provider = FixtureProvider::with_pages([Ok(response(
            Vec::new(),
            CalendarDeltaProgress::Complete(old_delta.clone()),
        ))]);
        let request_log = Arc::clone(&provider.requests);
        let workflow = CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        );
        workflow
            .sync_view(&view)
            .await
            .expect("the initial cursor commits");

        provider.push_page(Ok(CalendarDeltaResponse::Reset {
            reset_link: Some(reset_link.clone()),
            response_bytes: 24,
        }));
        provider.push_page(Ok(CalendarDeltaResponse::Reset {
            reset_link: None,
            response_bytes: 32,
        }));
        assert_eq!(
            workflow.sync_view(&view).await,
            Err(CalendarWorkflowError::DeltaResetLimitExceeded)
        );
        let checkpoint = workflow
            .load_cached_view(&view)
            .await
            .expect("the replacement checkpoint remains readable");
        assert_eq!(checkpoint.sync_state().delta_link(), Some(&old_delta));
        assert!(checkpoint.sync_state().replacement_active());
        assert_eq!(
            checkpoint.sync_state().request(),
            CalendarDeltaRequest::Initial
        );

        provider.push_page(Ok(response(
            Vec::new(),
            CalendarDeltaProgress::Complete(link("synthetic-recovered")),
        )));
        workflow
            .sync_view(&view)
            .await
            .expect("a later attempt resumes the fixed-range initial request");
        assert_eq!(
            *request_log
                .lock()
                .expect("synthetic request log mutex is not poisoned"),
            vec![
                CalendarDeltaRequest::Initial,
                CalendarDeltaRequest::Continue(old_delta),
                CalendarDeltaRequest::Continue(reset_link),
                CalendarDeltaRequest::Initial,
            ]
        );
    });
}

#[test]
fn event_writes_wait_for_an_interrupted_calendar_replacement_sync() {
    smol::block_on(async {
        let view = view();
        let another_view = view_for_week(8);
        let old_delta = link("synthetic-old-delta");
        let reset_link = link("synthetic-reset");
        let provider = FixtureProvider::with_pages([
            Ok(response(
                vec![CalendarDeltaChange::Upsert {
                    id: event_id(),
                    patch: cross_week_timed_event_patch(),
                }],
                CalendarDeltaProgress::Complete(old_delta.clone()),
            )),
            Ok(response(
                vec![CalendarDeltaChange::Upsert {
                    id: event_id(),
                    patch: cross_week_timed_event_patch(),
                }],
                CalendarDeltaProgress::Complete(link("synthetic-another-view")),
            )),
        ]);
        let workflow = CalendarWorkflow::new(
            workflow_account_id(),
            provider.clone(),
            FixtureCache::default(),
        );

        workflow
            .sync_view(&view)
            .await
            .expect("the initial event snapshot commits");
        workflow
            .sync_view(&another_view)
            .await
            .expect("the overlapping event view commits");
        let event = workflow
            .load_cached_view(&view)
            .await
            .expect("the initial event is readable")
            .events()[0]
            .clone();
        let another_event = workflow
            .load_cached_view(&another_view)
            .await
            .expect("the overlapping event is readable")
            .events()[0]
            .clone();
        let editable = workflow
            .load_versioned_event(&event)
            .await
            .expect("the synchronized event can be opened for editing");
        let draft = CalendarEventUpdateDraft::timed(
            "Updated planning".to_owned(),
            utc(2030, Month::January, 8) + time::Duration::hours(11),
            utc(2030, Month::January, 8) + time::Duration::hours(12),
        )
        .expect("the synthetic update draft is valid");
        let update = CalendarEventUpdate::new(editable.clone(), draft)
            .expect("the update differs from the source event");

        provider.push_page(Ok(CalendarDeltaResponse::Reset {
            reset_link: Some(reset_link),
            response_bytes: 64,
        }));
        provider.push_page(Err(CalendarOperationError::ServiceUnavailable));
        assert_eq!(
            workflow.sync_view(&view).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::ServiceUnavailable
            ))
        );

        assert_eq!(
            workflow
                .load_cached_view(&view)
                .await
                .expect("the interrupted replacement remains readable")
                .write_readiness(),
            CalendarWriteReadiness::AccountSyncRequired
        );
        assert_eq!(
            workflow
                .load_cached_view(&another_view)
                .await
                .expect("the other account view remains readable")
                .write_readiness(),
            CalendarWriteReadiness::AccountSyncRequired
        );
        assert_eq!(
            workflow.create_event(&creation(another_view.clone())).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert_eq!(
            workflow.load_versioned_event(&another_event).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert_eq!(
            workflow.update_event(&update).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert_eq!(
            workflow.delete_event(&editable).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
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
                .expect("synthetic deletion log mutex is not poisoned"),
            0
        );
    });
}
