use super::*;

#[test]
fn read_state_write_reserves_folder_until_graph_confirmation_and_cache_commit() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Synthetic message".to_owned(), started_at),
            }],
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-initial"
                    .to_owned(),
            )),
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            [Ok(page_response(page))],
            Err(MailOperationError::ServiceUnavailable),
        );
        let (update_started, release_update) = provider.hold_next_read_state();
        let updates = Arc::clone(&provider.read_state_updates);
        let requests = Arc::clone(&provider.requests);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = Arc::new(MailWorkflow::new(account.clone(), provider, cache.clone()));
        workflow
            .synchronize_account(started_at, &ignore_sync_progress)
            .await
            .expect("synthetic message is cached before its read-state action");

        let executor = smol::Executor::new();
        let update_workflow = Arc::clone(&workflow);
        let update_folder = folder_reference.clone();
        let (update_result, sync_result) = smol::block_on(executor.run(async {
            let update_task = executor.spawn(async move {
                update_workflow
                    .set_message_read_state(&update_folder, &message_id(), ReadState::Read)
                    .await
            });
            update_started
                .recv()
                .await
                .expect("synthetic provider waits before confirming the write");
            let sync_result = workflow
                .synchronize_account(started_at + Duration::from_secs(1), &ignore_sync_progress)
                .await;
            release_update
                .send(())
                .await
                .expect("synthetic provider releases its confirmation");
            (update_task.await, sync_result)
        }));

        assert_eq!(
            update_result.expect("read-state write completes"),
            einfach_mail_application::ReadStateUpdateOutcome::ConfirmedAndCached(ReadState::Read)
        );
        assert_eq!(sync_result, Err(MailWorkflowError::OperationInProgress));
        assert_eq!(
            requests
                .lock()
                .expect("synthetic request log mutex is healthy")
                .len(),
            1
        );
        assert_eq!(
            updates
                .lock()
                .expect("synthetic read-state log mutex is healthy")
                .as_slice(),
            [(message_id(), ReadState::Read)]
        );
        assert_eq!(
            cache
                .message_read_state(&folder_reference, &message_id())
                .await
                .expect("confirmed state can be read from the encrypted cache"),
            Some(ReadState::Read)
        );

        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn follow_up_flag_write_reserves_folder_until_confirmation_and_atomic_cache_commit() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut patch = complete_patch("Synthetic message".to_owned(), started_at);
        patch.set_follow_up_flag_status(Some(FollowUpFlagStatus::NotFlagged));
        let page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch,
            }],
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-initial"
                    .to_owned(),
            )),
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            [Ok(page_response(page))],
            Err(MailOperationError::ServiceUnavailable),
        );
        let (update_started, release_update) = provider.hold_next_follow_up_flag();
        let updates = Arc::clone(&provider.follow_up_flag_updates);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = Arc::new(MailWorkflow::new(account.clone(), provider, cache.clone()));
        workflow
            .synchronize_account(started_at, &ignore_sync_progress)
            .await
            .expect("synthetic message is cached before its flag action");

        let executor = smol::Executor::new();
        let update_workflow = Arc::clone(&workflow);
        let update_folder = folder_reference.clone();
        let (update_result, sync_result, staged_marker) = smol::block_on(executor.run(async {
            let update_task = executor.spawn(async move {
                update_workflow
                    .set_message_follow_up_flag_status(
                        &update_folder,
                        &message_id(),
                        FollowUpFlagStatus::Flagged,
                    )
                    .await
            });
            update_started
                .recv()
                .await
                .expect("synthetic provider waits before confirming the flag write");
            let staged_marker = cache
                .folder_write_sync_required(&folder_reference)
                .await
                .expect("recovery marker is readable while the PATCH is pending");
            let sync_result = workflow
                .synchronize_account(started_at + Duration::from_secs(1), &ignore_sync_progress)
                .await;
            release_update
                .send(())
                .await
                .expect("synthetic provider releases its confirmation");
            (update_task.await, sync_result, staged_marker)
        }));

        assert!(staged_marker);
        assert_eq!(sync_result, Err(MailWorkflowError::OperationInProgress));
        assert_eq!(
            update_result.expect("flag update completes"),
            einfach_mail_application::FollowUpFlagUpdateOutcome::ConfirmedAndCached(
                FollowUpFlagStatus::Flagged
            )
        );
        assert!(
            !cache
                .folder_write_sync_required(&folder_reference)
                .await
                .expect("confirmed cache update clears its recovery marker")
        );
        assert_eq!(
            cache
                .read_message(&folder_reference, &message_id())
                .await
                .expect("updated message remains readable")
                .and_then(|(message, _)| message.metadata().follow_up_flag_status()),
            Some(FollowUpFlagStatus::Flagged)
        );
        assert_eq!(
            updates
                .lock()
                .expect("synthetic flag update log mutex is healthy")
                .as_slice(),
            [(message_id(), FollowUpFlagStatus::Flagged)]
        );

        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn definite_follow_up_flag_rejection_clears_marker_without_changing_cached_status() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut patch = complete_patch("Synthetic message".to_owned(), started_at);
        patch.set_follow_up_flag_status(Some(FollowUpFlagStatus::NotFlagged));
        let page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch,
            }],
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-initial"
                    .to_owned(),
            )),
        );
        let mut provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            [Ok(page_response(page))],
            Err(MailOperationError::ServiceUnavailable),
        );
        provider.follow_up_flag_response = FollowUpFlagResponse::Deny;
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(account.clone(), provider, cache.clone());
        workflow
            .synchronize_account(started_at, &ignore_sync_progress)
            .await
            .expect("synthetic message is cached before its flag action");

        assert_eq!(
            workflow
                .set_message_follow_up_flag_status(
                    &folder_reference,
                    &message_id(),
                    FollowUpFlagStatus::Flagged,
                )
                .await,
            Err(MailWorkflowError::Provider(
                MailOperationError::AccessDenied
            ))
        );
        assert!(
            !cache
                .folder_write_sync_required(&folder_reference)
                .await
                .expect("definite rejection clears its recovery marker")
        );
        assert_eq!(
            cache
                .read_message(&folder_reference, &message_id())
                .await
                .expect("cached state remains readable after rejection")
                .and_then(|(message, _)| message.metadata().follow_up_flag_status()),
            Some(FollowUpFlagStatus::NotFlagged)
        );

        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn unknown_follow_up_flag_outcome_survives_reopen_until_a_complete_delta_round() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut patch = complete_patch("Synthetic message".to_owned(), started_at);
        patch.set_follow_up_flag_status(Some(FollowUpFlagStatus::NotFlagged));
        let initial_page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch,
            }],
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-initial"
                    .to_owned(),
            )),
        );
        let mut provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder.clone()],
            [Ok(page_response(initial_page))],
            Err(MailOperationError::ServiceUnavailable),
        );
        provider.follow_up_flag_response = FollowUpFlagResponse::Unknown;
        let updates = Arc::clone(&provider.follow_up_flag_updates);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(account.clone(), provider, cache.clone());
        workflow
            .synchronize_account(started_at, &ignore_sync_progress)
            .await
            .expect("synthetic message is cached before its flag action");

        assert_eq!(
            workflow
                .set_message_follow_up_flag_status(
                    &folder_reference,
                    &message_id(),
                    FollowUpFlagStatus::Flagged,
                )
                .await,
            Err(MailWorkflowError::Provider(
                MailOperationError::MessageUpdateOutcomeUnknown
            ))
        );
        assert!(
            cache
                .folder_write_sync_required(&folder_reference)
                .await
                .expect("unknown outcome keeps the recovery marker")
        );
        assert_eq!(
            workflow
                .set_message_follow_up_flag_status(
                    &folder_reference,
                    &message_id(),
                    FollowUpFlagStatus::Flagged,
                )
                .await,
            Err(MailWorkflowError::FolderSyncRequired)
        );
        assert_eq!(
            updates
                .lock()
                .expect("synthetic flag update log mutex is healthy")
                .len(),
            1
        );
        drop(workflow);
        cache.close_for_tests().await;

        let reopened = open_cache(&directory, account.clone()).await;
        assert!(
            reopened
                .folder_write_sync_required(&folder_reference)
                .await
                .expect("recovery marker survives cache reopen")
        );
        let final_page = MessageDeltaPage::new(
            Vec::new(),
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-reconciled"
                    .to_owned(),
            )),
        );
        let recovery_workflow = MailWorkflow::new(
            account.clone(),
            FixtureProvider::new(
                account.clone(),
                vec![cached_folder],
                [Ok(page_response(final_page))],
                Err(MailOperationError::ServiceUnavailable),
            ),
            reopened.clone(),
        );
        recovery_workflow
            .synchronize_account(started_at + Duration::from_secs(1), &ignore_sync_progress)
            .await
            .expect("a complete delta round reconciles the unknown update");
        assert!(
            !reopened
                .folder_write_sync_required(&folder_reference)
                .await
                .expect("completed delta round clears the recovery marker")
        );
        assert_eq!(
            reopened
                .read_message(&folder_reference, &message_id())
                .await
                .expect("message remains readable after reconciliation")
                .and_then(|(message, _)| message.metadata().follow_up_flag_status()),
            Some(FollowUpFlagStatus::NotFlagged)
        );

        drop(recovery_workflow);
        reopened.close_for_tests().await;
    });
}

#[test]
fn failure_to_commit_flag_recovery_marker_prevents_provider_dispatch() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Synthetic message".to_owned(), started_at),
            }],
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-initial"
                    .to_owned(),
            )),
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            [Ok(page_response(page))],
            Err(MailOperationError::ServiceUnavailable),
        );
        let updates = Arc::clone(&provider.follow_up_flag_updates);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(account.clone(), provider, cache.clone());
        workflow
            .synchronize_account(started_at, &ignore_sync_progress)
            .await
            .expect("synthetic message is cached before the flag action");
        cache
            .execute_batch_for_tests(
                "CREATE TRIGGER fail_flag_marker_insert
                 BEFORE INSERT ON mail_folder_write_sync_requirement
                 BEGIN SELECT RAISE(ABORT, 'synthetic marker failure'); END;",
            )
            .await
            .expect("synthetic failure trigger is installed");

        assert_eq!(
            workflow
                .set_message_follow_up_flag_status(
                    &folder_reference,
                    &message_id(),
                    FollowUpFlagStatus::Flagged,
                )
                .await,
            Err(MailWorkflowError::Cache(MailCacheError::Unavailable))
        );
        assert!(
            updates
                .lock()
                .expect("synthetic flag update log mutex is healthy")
                .is_empty()
        );
        assert!(
            !cache
                .folder_write_sync_required(&folder_reference)
                .await
                .expect("failed marker transaction leaves no recovery marker")
        );

        drop(workflow);
        cache.close_for_tests().await;
    });
}
