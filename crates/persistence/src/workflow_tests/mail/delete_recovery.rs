use super::*;

#[test]
fn unknown_delete_outcome_requires_sync_before_another_move_request() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account);
        let deleted = deleted_items_folder(&account);
        let inbox_reference = inbox.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let source_page = || {
            completed_message_delta_page(
                "unknown-delete-source",
                vec![MessageDeltaChange::Upsert {
                    id: message_id(),
                    patch: complete_patch("Synthetic subject".to_owned(), started_at),
                }],
            )
        };
        let provider = FixtureProvider::new(
            account.clone(),
            vec![inbox, deleted],
            [
                Ok(page_response(source_page())),
                Ok(page_response(completed_message_delta_page(
                    "unknown-delete-destination",
                    Vec::new(),
                ))),
                Ok(page_response(source_page())),
                Ok(page_response(completed_message_delta_page(
                    "unknown-delete-resync-destination",
                    Vec::new(),
                ))),
            ],
            Err(MailOperationError::ServiceUnavailable),
        );
        *provider
            .move_result
            .lock()
            .expect("synthetic move result mutex is not poisoned") =
            Err(MailOperationError::MoveOutcomeUnknown);
        let move_calls = Arc::clone(&provider.message_moves);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(account.clone(), provider.clone(), cache.clone());
        workflow
            .synchronize_account(started_at, &ignore_sync_progress)
            .await
            .expect("synthetic account sync completes");

        assert_eq!(
            workflow
                .move_message_to_well_known_folder(
                    &inbox_reference,
                    &message_id(),
                    WellKnownMailFolder::DeletedItems
                )
                .await,
            Err(MailWorkflowError::Provider(
                MailOperationError::MoveOutcomeUnknown
            ))
        );
        assert!(
            cache
                .read_message(&inbox_reference, &message_id())
                .await
                .expect("source cache lookup succeeds after unknown outcome")
                .is_some()
        );
        assert_eq!(
            workflow
                .move_message_to_well_known_folder(
                    &inbox_reference,
                    &message_id(),
                    WellKnownMailFolder::DeletedItems
                )
                .await,
            Err(MailWorkflowError::AccountSyncRequired)
        );
        assert_eq!(
            move_calls
                .lock()
                .expect("synthetic move log mutex is not poisoned")
                .len(),
            1
        );

        workflow
            .synchronize_account(started_at + Duration::from_secs(1), &ignore_sync_progress)
            .await
            .expect("a new complete account sync restores message writes");
        *provider
            .move_result
            .lock()
            .expect("synthetic move result mutex is not poisoned") = Ok(());
        assert_eq!(
            workflow
                .move_message_to_well_known_folder(
                    &inbox_reference,
                    &message_id(),
                    WellKnownMailFolder::DeletedItems,
                )
                .await
                .map(|outcome| outcome.cache_outcome()),
            Ok(einfach_mail_application::MessageMoveOutcome::ConfirmedAndCached)
        );
        assert_eq!(
            move_calls
                .lock()
                .expect("synthetic move log mutex is not poisoned")
                .len(),
            2
        );
        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn canceled_delete_request_requires_sync_before_another_move_request() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account);
        let deleted = deleted_items_folder(&account);
        let inbox_reference = inbox.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let source_page = completed_message_delta_page(
            "canceled-delete-source",
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Synthetic subject".to_owned(), started_at),
            }],
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![inbox, deleted],
            [
                Ok(page_response(source_page)),
                Ok(page_response(completed_message_delta_page(
                    "canceled-delete-destination",
                    Vec::new(),
                ))),
            ],
            Err(MailOperationError::ServiceUnavailable),
        );
        let (move_started_receiver, _release_move) = provider.hold_next_message_move();
        let move_calls = Arc::clone(&provider.message_moves);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(account.clone(), provider, cache.clone());
        workflow
            .synchronize_account(started_at, &ignore_sync_progress)
            .await
            .expect("synthetic account sync completes");

        smol::future::race(
            async {
                let _ = workflow
                    .move_message_to_well_known_folder(
                        &inbox_reference,
                        &message_id(),
                        WellKnownMailFolder::DeletedItems,
                    )
                    .await;
            },
            async {
                move_started_receiver
                    .recv()
                    .await
                    .expect("provider move reaches its cancellation gate");
            },
        )
        .await;

        assert_eq!(
            workflow
                .move_message_to_well_known_folder(
                    &inbox_reference,
                    &message_id(),
                    WellKnownMailFolder::DeletedItems
                )
                .await,
            Err(MailWorkflowError::AccountSyncRequired)
        );
        assert_eq!(
            move_calls
                .lock()
                .expect("synthetic move log mutex is not poisoned")
                .len(),
            1
        );
        assert!(
            cache
                .read_message(&inbox_reference, &message_id())
                .await
                .expect("source cache lookup succeeds after cancellation")
                .is_some()
        );
        cache.close_for_tests().await;
    });
}

#[test]
fn delete_message_keeps_the_source_reservation_while_resolving_deleted_items() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account);
        let archive = archive_folder(&account);
        let deleted = deleted_items_folder(&account);
        let inbox_reference = inbox.reference().clone();
        let archive_reference = archive.reference().clone();
        let deleted_reference = deleted.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let provider = FixtureProvider::new(
            account.clone(),
            vec![inbox, archive, deleted],
            [
                Ok(page_response(completed_message_delta_page(
                    "reserved-delete-source",
                    vec![MessageDeltaChange::Upsert {
                        id: message_id(),
                        patch: complete_patch("Synthetic subject".to_owned(), started_at),
                    }],
                ))),
                Ok(page_response(completed_message_delta_page(
                    "reserved-delete-archive",
                    Vec::new(),
                ))),
                Ok(page_response(completed_message_delta_page(
                    "reserved-delete-destination",
                    Vec::new(),
                ))),
            ],
            Err(MailOperationError::ServiceUnavailable),
        );
        let (lookup_started, release_lookup) = provider.hold_next_well_known_folder_lookup();
        let move_calls = Arc::clone(&provider.message_moves);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = Arc::new(MailWorkflow::new(account.clone(), provider, cache.clone()));
        workflow
            .synchronize_account(started_at, &ignore_sync_progress)
            .await
            .expect("synthetic account sync completes");
        let executor = smol::Executor::new();
        let delete_workflow = Arc::clone(&workflow);
        let delete_source = inbox_reference.clone();

        let (delete_result, competing_move) = smol::block_on(executor.run(async {
            let delete_task = executor.spawn(async move {
                delete_workflow
                    .move_message_to_well_known_folder(
                        &delete_source,
                        &message_id(),
                        WellKnownMailFolder::DeletedItems,
                    )
                    .await
            });
            lookup_started
                .recv()
                .await
                .expect("Deleted Items resolver blocks after source reservation");
            let competing_move = workflow
                .move_message(&inbox_reference, &message_id(), &archive_reference)
                .await;
            release_lookup
                .send(())
                .await
                .expect("Deleted Items resolver gate remains open");
            (delete_task.await, competing_move)
        }));

        assert_eq!(competing_move, Err(MailWorkflowError::OperationInProgress));
        assert_eq!(
            delete_result.map(|outcome| {
                (
                    outcome.destination_folder().clone(),
                    outcome.cache_outcome(),
                )
            }),
            Ok((
                deleted_reference,
                einfach_mail_application::MessageMoveOutcome::ConfirmedAndCached,
            ))
        );
        assert_eq!(
            move_calls
                .lock()
                .expect("synthetic move log mutex is not poisoned")
                .len(),
            1
        );
        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn confirmed_delete_with_cache_failure_requires_sync_before_retry() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account);
        let deleted = deleted_items_folder(&account);
        let inbox_reference = inbox.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let source_page = completed_message_delta_page(
            "failed-cache-delete-source",
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Synthetic subject".to_owned(), started_at),
            }],
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![inbox, deleted],
            [
                Ok(page_response(source_page)),
                Ok(page_response(completed_message_delta_page(
                    "failed-cache-delete-destination",
                    Vec::new(),
                ))),
            ],
            Err(MailOperationError::ServiceUnavailable),
        );
        let folder_calls = Arc::clone(&provider.well_known_folder_calls);
        let move_calls = Arc::clone(&provider.message_moves);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(account.clone(), provider, cache.clone());
        workflow
            .synchronize_account(started_at, &ignore_sync_progress)
            .await
            .expect("synthetic account sync completes");
        cache.set_message_move_failure_for_tests(true).await;

        assert_eq!(
            workflow
                .move_message_to_well_known_folder(
                    &inbox_reference,
                    &message_id(),
                    WellKnownMailFolder::DeletedItems,
                )
                .await
                .map(|outcome| outcome.cache_outcome()),
            Ok(einfach_mail_application::MessageMoveOutcome::ConfirmedNeedsSync)
        );
        assert_eq!(
            workflow
                .move_message_to_well_known_folder(
                    &inbox_reference,
                    &message_id(),
                    WellKnownMailFolder::DeletedItems
                )
                .await,
            Err(MailWorkflowError::AccountSyncRequired)
        );
        assert_eq!(folder_calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            move_calls
                .lock()
                .expect("synthetic move log mutex is not poisoned")
                .len(),
            1
        );
        assert!(
            cache
                .read_message(&inbox_reference, &message_id())
                .await
                .expect("source cache lookup succeeds after transaction rollback")
                .is_some()
        );
        cache.close_for_tests().await;
    });
}
