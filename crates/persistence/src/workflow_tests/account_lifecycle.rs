use super::*;

#[test]
fn account_cache_removal_preserves_the_in_use_error() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let first = open_cache(&directory, account.clone()).await;
        let second = open_cache(&directory, account.clone()).await;

        assert_eq!(
            <SqliteMailCache as AccountCache>::remove_account(&first, &account).await,
            Err(AccountCacheRemovalError::InUse)
        );

        first.close_for_tests().await;
        second.close_for_tests().await;
    });
}

#[test]
fn factory_sessions_share_folder_reservations_during_body_fetch() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let cutoff = started_at - Duration::from_secs(90 * 24 * 60 * 60);
        let cache = open_cache(&directory, account.clone()).await;
        cache
            .replace_folders(&account, vec![cached_folder.clone()])
            .await
            .expect("synthetic folder is cached");
        cache
            .ensure_account_sync_scope(&account, cutoff)
            .await
            .expect("account cutoff is saved");
        let initial_state = cache
            .ensure_sync_state(&folder_reference, cutoff)
            .await
            .expect("initial checkpoint is saved");
        cache
            .commit_delta_page(
                &folder_reference,
                &initial_state,
                MessageDeltaPage::new(
                    vec![MessageDeltaChange::Upsert {
                        id: message_id(),
                        patch: complete_patch("Cached message".to_owned(), started_at),
                    }],
                    MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                        "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken=baseline"
                            .to_owned(),
                    )),
                ),
                started_at,
            )
            .await
            .expect("synthetic baseline sync commits");
        cache.close_for_tests().await;

        let (started_sender, started_receiver) = smol::channel::unbounded();
        let (release_sender, release_receiver) = smol::channel::bounded(1);
        let mut provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            [],
            Ok(MessageBody::new(
                MessageBodyType::Text,
                "Synthetic fetched body".to_owned(),
            )),
        );
        provider.body_gate = Some(BodyFetchGate {
            started: started_sender,
            release: release_receiver,
        });
        let requests = Arc::clone(&provider.requests);
        let factory = AccountSessionWorkflowFactory::new(
            provider,
            FixtureCacheFactory {
                database_directory: directory.path().to_path_buf(),
            },
        );
        let opening_session = factory
            .open_session(account.clone())
            .await
            .expect("first mail session opens");
        let syncing_session = factory
            .open_session(account.clone())
            .await
            .expect("second mail session opens");
        let executor = smol::Executor::new();
        let opening_session_task = Arc::clone(&opening_session);
        let opening_folder = folder_reference.clone();
        let opening_message = message_id();

        let (open_result, sync_result) = smol::block_on(executor.run(async {
            let open_task = executor.spawn(async move {
                opening_session_task
                    .mail_session()
                    .open_message(&opening_folder, &opening_message)
                    .await
            });
            started_receiver
                .recv()
                .await
                .expect("body fetch reaches the synchronization gate");
            let sync_result = syncing_session
                .mail_session()
                .synchronize_account(started_at + Duration::from_secs(1), &ignore_sync_progress)
                .await;
            release_sender
                .send(())
                .await
                .expect("body fetch gate is still open");
            (open_task.await, sync_result)
        }));

        assert!(
            !open_result
                .expect("message open completes")
                .loaded_from_cache()
        );
        assert_eq!(sync_result, Err(MailWorkflowError::OperationInProgress));
        assert!(
            requests
                .lock()
                .expect("synthetic request log mutex is not poisoned")
                .is_empty()
        );
        drop(opening_session);
        drop(syncing_session);
    });
}

#[test]
fn factory_sessions_share_uncertain_message_move_recovery() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account);
        let archive = archive_folder(&account);
        let inbox_reference = inbox.reference().clone();
        let archive_reference = archive.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let pages = [
            Ok(page_response(completed_message_delta_page(
                "shared-move-inbox",
                vec![MessageDeltaChange::Upsert {
                    id: message_id(),
                    patch: complete_patch("Synthetic subject".to_owned(), started_at),
                }],
            ))),
            Ok(page_response(completed_message_delta_page(
                "shared-move-archive",
                Vec::new(),
            ))),
            Ok(page_response(completed_message_delta_page(
                "shared-move-resync-inbox",
                Vec::new(),
            ))),
            Ok(page_response(completed_message_delta_page(
                "shared-move-resync-archive",
                Vec::new(),
            ))),
        ];
        let provider = FixtureProvider::new(
            account.clone(),
            vec![inbox, archive],
            pages,
            Err(MailOperationError::ServiceUnavailable),
        );
        let move_calls = Arc::clone(&provider.message_moves);
        let factory = AccountSessionWorkflowFactory::new(
            provider.clone(),
            FixtureCacheFactory {
                database_directory: directory.path().to_path_buf(),
            },
        );
        let first = factory
            .open_session(account.clone())
            .await
            .expect("first account session opens");
        let second = factory
            .open_session(account.clone())
            .await
            .expect("second account session opens");
        first
            .mail_session()
            .synchronize_account(started_at, &ignore_sync_progress)
            .await
            .expect("first account sync completes");
        *provider
            .move_result
            .lock()
            .expect("synthetic move result mutex is not poisoned") =
            Err(MailOperationError::MoveOutcomeUnknown);

        assert_eq!(
            first
                .mail_session()
                .move_message(&inbox_reference, &message_id(), &archive_reference)
                .await,
            Err(MailWorkflowError::Provider(
                MailOperationError::MoveOutcomeUnknown
            ))
        );
        assert_eq!(
            second
                .mail_session()
                .move_message(&inbox_reference, &message_id(), &archive_reference)
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

        second
            .mail_session()
            .synchronize_account(started_at + Duration::from_secs(1), &ignore_sync_progress)
            .await
            .expect("second account sync restores shared mail readiness");
        *provider
            .move_result
            .lock()
            .expect("synthetic move result mutex is not poisoned") = Ok(());
        assert_eq!(
            second
                .mail_session()
                .move_message(&inbox_reference, &message_id(), &archive_reference)
                .await,
            Ok(einfach_mail_application::MessageMoveOutcome::ConfirmedAndCached)
        );
        drop(first);
        drop(second);
    });
}

#[test]
fn factory_sessions_share_uncertain_calendar_delete_recovery() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let view = calendar_view(&account);
        let provider = FixtureProvider::new(
            account.clone(),
            Vec::new(),
            [],
            Err(MailOperationError::ServiceUnavailable),
        )
        .with_calendar_pages([
            completed_calendar_page("calendar-session-first"),
            completed_calendar_page("calendar-session-second"),
        ])
        .with_calendar_delete_result(Err(CalendarOperationError::DeleteOutcomeUnknown));
        let delete_calls = Arc::clone(&provider.calendar_delete_calls);
        let factory = AccountSessionWorkflowFactory::new(
            provider,
            FixtureCacheFactory {
                database_directory: directory.path().to_path_buf(),
            },
        );
        let first = factory
            .open_session(account.clone())
            .await
            .expect("first account session opens");
        let second = factory
            .open_session(account)
            .await
            .expect("second account session opens");
        let first_calendar = first.calendar_session();
        let second_calendar = second.calendar_session();
        first_calendar
            .synchronize_view(&view)
            .await
            .expect("first calendar view syncs");
        second_calendar
            .synchronize_view(&view)
            .await
            .expect("second calendar view syncs");
        let event = editable_calendar_event(view);

        assert_eq!(
            first_calendar.delete_event(&event).await,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::DeleteOutcomeUnknown
            ))
        );
        assert_eq!(
            second_calendar.delete_event(&event).await,
            Err(CalendarWorkflowError::CalendarWriteSyncRequired)
        );
        assert_eq!(delete_calls.load(Ordering::Relaxed), 1);
    });
}

#[test]
fn calendar_sync_reservation_blocks_sign_out_cache_removal() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let (started_sender, started_receiver) = smol::channel::bounded(1);
        let (release_sender, release_receiver) = smol::channel::bounded(1);
        let mut provider = FixtureProvider::new(
            account.clone(),
            Vec::new(),
            [],
            Err(MailOperationError::ServiceUnavailable),
        );
        provider.calendar_gate = Some(CalendarSyncGate {
            started: started_sender,
            release: release_receiver,
        });
        let factory = AccountSessionWorkflowFactory::new(
            provider,
            FixtureCacheFactory {
                database_directory: directory.path().to_path_buf(),
            },
        );
        let session = factory
            .open_session(account.clone())
            .await
            .expect("account session opens");
        let calendar = session.calendar_session();
        let view = calendar_view(&account);
        let sync_calendar = Arc::clone(&calendar);
        let sync_view = view.clone();
        let sync_session = Arc::clone(&session);
        let executor = smol::Executor::new();

        let (sync_result, session_removal_result, factory_removal_result) = executor
            .run(async {
                let sync_task =
                    executor.spawn(async move { sync_calendar.synchronize_view(&sync_view).await });
                started_receiver
                    .recv()
                    .await
                    .expect("calendar request reaches the synthetic gate");
                let session_removal_result = sync_session.remove_account_cache().await;
                let factory_removal_result = factory.remove_account_cache(&account).await;
                release_sender
                    .send(())
                    .await
                    .expect("calendar sync gate is still open");
                (
                    sync_task.await,
                    session_removal_result,
                    factory_removal_result,
                )
            })
            .await;

        assert_eq!(
            sync_result,
            Err(CalendarWorkflowError::Provider(
                CalendarOperationError::ServiceUnavailable
            ))
        );
        assert_eq!(
            session_removal_result,
            Err(AccountCacheRemovalError::OperationInProgress)
        );
        assert_eq!(
            factory_removal_result,
            Err(AccountCacheRemovalError::OperationInProgress)
        );
        calendar
            .load_cached_view(&view)
            .await
            .expect("the shared cache remains available after sign-out is blocked");
    });
}

#[test]
fn account_session_removes_cache_through_its_open_cache_handle() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let provider = FixtureProvider::new(
            account.clone(),
            Vec::new(),
            [],
            Err(MailOperationError::ServiceUnavailable),
        );
        let factory = AccountSessionWorkflowFactory::new(
            provider,
            FixtureCacheFactory {
                database_directory: directory.path().to_path_buf(),
            },
        );
        let session = factory
            .open_session(account.clone())
            .await
            .expect("account session opens");

        assert_removal_completed(
            session.remove_account_cache().await,
            &directory.path().join("mail.sqlite3"),
        );
    });
}

#[test]
fn account_session_rejects_foreign_references_without_provider_access() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let other_account = AccountId::try_from("synthetic-other-account".to_owned())
            .expect("synthetic account ID is valid");
        let provider = FixtureProvider::new(
            account.clone(),
            Vec::new(),
            [],
            Err(MailOperationError::ServiceUnavailable),
        )
        .with_calendar_pages([completed_calendar_page("untouched-foreign-view")]);
        let well_known_folder_calls = Arc::clone(&provider.well_known_folder_calls);
        let calendar_pages = Arc::clone(&provider.calendar_pages);
        let factory = AccountSessionWorkflowFactory::new(
            provider,
            FixtureCacheFactory {
                database_directory: directory.path().to_path_buf(),
            },
        );
        let session = factory
            .open_session(account)
            .await
            .expect("account session opens");
        let foreign_folder = folder(&other_account).reference().clone();
        let foreign_view = calendar_view(&other_account);

        assert_eq!(
            session
                .mail_session()
                .list_cached_messages(&foreign_folder)
                .await,
            Err(MailWorkflowError::AccountMismatch)
        );
        assert_eq!(
            session
                .mail_session()
                .move_message_to_well_known_folder(
                    &foreign_folder,
                    &message_id(),
                    WellKnownMailFolder::Archive,
                )
                .await,
            Err(MailWorkflowError::AccountMismatch)
        );
        assert_eq!(
            session
                .calendar_session()
                .synchronize_view(&foreign_view)
                .await,
            Err(CalendarWorkflowError::AccountMismatch)
        );
        assert_eq!(
            well_known_folder_calls.load(Ordering::Relaxed),
            0,
            "foreign mail references do not reach the provider"
        );
        assert_eq!(
            calendar_pages
                .lock()
                .expect("synthetic calendar page queue is healthy")
                .len(),
            1,
            "foreign calendar views do not reach the provider"
        );
    });
}

#[test]
fn factory_account_reservation_prevents_folder_snapshot_replacement_race() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Account snapshot survives".to_owned(), started_at),
            }],
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken=complete"
                    .to_owned(),
            )),
        );
        let mut provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder.clone()],
            [Ok(page_response(page))],
            Err(MailOperationError::ServiceUnavailable),
        );
        provider
            .folder_snapshots
            .lock()
            .expect("synthetic folder snapshot mutex is not poisoned")
            .extend([vec![cached_folder.clone()], Vec::new()]);
        let (started_sender, started_receiver) = smol::channel::unbounded();
        let (release_sender, release_receiver) = smol::channel::bounded(1);
        provider.delta_gate = Some(DeltaPageGate {
            request_count: Arc::new(AtomicUsize::new(0)),
            request_index_to_block: 0,
            started: started_sender,
            release: release_receiver,
        });
        let folder_list_calls = Arc::clone(&provider.folder_list_calls);
        let factory = AccountSessionWorkflowFactory::new(
            provider,
            FixtureCacheFactory {
                database_directory: directory.path().to_path_buf(),
            },
        );
        let syncing_session = factory
            .open_session(account.clone())
            .await
            .expect("first mail session opens");
        let competing_session = factory
            .open_session(account.clone())
            .await
            .expect("second mail session opens");
        let executor = smol::Executor::new();
        let (sync_result, competing_result) = smol::block_on(executor.run(async {
            let sync_task = executor.spawn(async move {
                syncing_session
                    .mail_session()
                    .synchronize_account(started_at, &ignore_sync_progress)
                    .await
            });
            started_receiver
                .recv()
                .await
                .expect("first delta request reaches its gate");
            let competing_result = competing_session
                .mail_session()
                .synchronize_account(started_at + Duration::from_secs(1), &ignore_sync_progress)
                .await;
            release_sender
                .send(())
                .await
                .expect("first delta request gate is still open");
            (sync_task.await, competing_result)
        }));

        let report = sync_result.expect("first account synchronization completes");
        assert_eq!(report.folder_count(), 1);
        assert_eq!(
            competing_result,
            Err(MailWorkflowError::OperationInProgress),
            "a competing sync is rejected before it can replace the folder snapshot"
        );
        assert_eq!(folder_list_calls.load(Ordering::Relaxed), 1);
        let folders = competing_session
            .mail_session()
            .list_cached_folders()
            .await
            .expect("saved folder snapshot remains readable");
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].reference(), &folder_reference);
        let messages = competing_session
            .mail_session()
            .list_cached_messages(&folder_reference)
            .await
            .expect("saved message snapshot remains readable");
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0].metadata().subject(),
            Some("Account snapshot survives")
        );
    });
}

#[test]
fn account_sync_reservation_blocks_moves_before_folder_tree_replacement() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account);
        let archive = archive_folder(&account);
        let inbox_reference = inbox.reference().clone();
        let archive_reference = archive.reference().clone();
        let empty_page = || {
            page_response(MessageDeltaPage::new(
                Vec::new(),
                MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                    "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-empty"
                        .to_owned(),
                )),
            ))
        };
        let mut provider = FixtureProvider::new(
            account.clone(),
            vec![inbox, archive],
            [Ok(empty_page()), Ok(empty_page())],
            Err(MailOperationError::ServiceUnavailable),
        );
        let (folder_list_started, folder_list_started_rx) = smol::channel::unbounded();
        let (release_folder_list_tx, release_folder_list) = smol::channel::unbounded();
        provider.folder_list_gate = Some((folder_list_started, release_folder_list));
        let move_calls = Arc::clone(&provider.message_moves);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = Arc::new(MailWorkflow::new(account.clone(), provider, cache.clone()));
        let sync_workflow = Arc::clone(&workflow);
        let sync_task = smol::spawn(async move {
            sync_workflow
                .synchronize_account(
                    UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                    &ignore_sync_progress,
                )
                .await
        });
        folder_list_started_rx
            .recv()
            .await
            .expect("account sync reaches folder listing");

        assert_eq!(
            workflow
                .move_message(&inbox_reference, &message_id(), &archive_reference)
                .await,
            Err(MailWorkflowError::OperationInProgress)
        );
        assert!(
            move_calls
                .lock()
                .expect("synthetic move log mutex is not poisoned")
                .is_empty()
        );

        release_folder_list_tx
            .send(())
            .await
            .expect("folder listing gate is still open");
        sync_task
            .await
            .expect("account synchronization owns the folder tree and its delta feeds");
        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn stale_cache_handle_cannot_stage_a_send_after_account_cache_removal() {
    smol::block_on(async {
        let directory = TempDir::new().expect("synthetic mail cache directory is created");
        let account = account_id();
        let removal_cache = open_cache(&directory, account.clone()).await;
        let stale_send_cache = open_cache(&directory, account.clone()).await;
        removal_cache.close_for_tests().await;
        drop(removal_cache);
        std::fs::remove_file(directory.path().join("mail.sqlite3"))
            .expect("cache replacement unlinks the database before the stale handle stages a send");

        let provider = FixtureProvider::new(
            account.clone(),
            Vec::new(),
            [],
            Err(MailOperationError::ServiceUnavailable),
        )
        .with_send_result(Ok(()));
        let send_calls = Arc::clone(&provider.send_calls);
        let workflow = MailWorkflow::new(account.clone(), provider, stale_send_cache.clone());

        assert_eq!(
            workflow
                .send_message(
                    draft("Stale cache subject", "stale cache body"),
                    UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                )
                .await,
            Err(MailSendWorkflowError::Cache(MailCacheError::Unavailable))
        );
        assert_eq!(send_calls.load(Ordering::Relaxed), 0);

        drop(workflow);
        stale_send_cache.close_for_tests().await;
    });
}

#[test]
fn account_removal_reservation_blocks_new_mail_work_before_provider_calls() {
    smol::block_on(async {
        let directory = TempDir::new().expect("synthetic account directory is created");
        let account = account_id();
        let inbox = folder(&account);
        let archive = archive_folder(&account);
        let inbox_reference = inbox.reference().clone();
        let archive_reference = archive.reference().clone();
        let mut provider = FixtureProvider::new(
            account.clone(),
            vec![inbox, archive],
            [],
            Err(MailOperationError::ServiceUnavailable),
        );
        provider.send_result = Ok(());
        let folder_list_calls = Arc::clone(&provider.folder_list_calls);
        let send_calls = Arc::clone(&provider.send_calls);
        let read_state_updates = Arc::clone(&provider.read_state_updates);
        let message_moves = Arc::clone(&provider.message_moves);
        let cache = open_cache(&directory, account.clone()).await;
        let session = open_session_over_retained_cache(provider, &account, &cache).await;
        let workflow = session.mail_session();
        let (marker_check_finished, resume_removal) =
            cache.hold_removal_after_marker_check_for_tests();
        let removing_session = Arc::clone(&session);
        let removal_task =
            smol::spawn(async move { removing_session.remove_account_cache().await });
        marker_check_finished
            .recv()
            .await
            .expect("removal checked the empty pending-send marker");

        assert_eq!(
            workflow
                .synchronize_account(
                    UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                    &ignore_sync_progress,
                )
                .await,
            Err(MailWorkflowError::OperationInProgress)
        );
        assert_eq!(
            workflow
                .set_message_read_state(&inbox_reference, &message_id(), ReadState::Read)
                .await,
            Err(MailWorkflowError::OperationInProgress)
        );
        assert_eq!(
            workflow
                .move_message(&inbox_reference, &message_id(), &archive_reference)
                .await,
            Err(MailWorkflowError::OperationInProgress)
        );
        assert_eq!(
            workflow
                .send_message(
                    draft("Synthetic subject", "Synthetic body"),
                    UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                )
                .await,
            Err(MailSendWorkflowError::OperationInProgress)
        );
        assert_eq!(folder_list_calls.load(Ordering::Relaxed), 0);
        assert_eq!(send_calls.load(Ordering::Relaxed), 0);
        assert!(
            read_state_updates
                .lock()
                .expect("synthetic read-state log mutex is healthy")
                .is_empty()
        );
        assert!(
            message_moves
                .lock()
                .expect("synthetic message-move log mutex is healthy")
                .is_empty()
        );

        resume_removal
            .send(())
            .await
            .expect("account removal gate is released");
        let removal_result = removal_task.await;
        assert_removal_completed(removal_result, &directory.path().join("mail.sqlite3"));
    });
}

#[test]
fn active_send_reservation_blocks_account_removal_before_marker_staging() {
    smol::block_on(async {
        let directory = TempDir::new().expect("synthetic account directory is created");
        let account = account_id();
        let mut provider = FixtureProvider::new(
            account.clone(),
            Vec::new(),
            [],
            Err(MailOperationError::ServiceUnavailable),
        );
        provider.send_result = Ok(());
        let send_calls = Arc::clone(&provider.send_calls);
        let cache = open_cache(&directory, account.clone()).await;
        let session = open_session_over_retained_cache(provider, &account, &cache).await;
        let workflow = session.mail_session();
        let (marker_read_started, release_marker_read) = cache.hold_pending_send_read_for_tests();
        let sending_workflow = Arc::clone(&workflow);
        let send_task = smol::spawn(async move {
            sending_workflow
                .send_message(
                    draft("Synthetic subject", "Synthetic body"),
                    UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                )
                .await
        });
        marker_read_started
            .recv()
            .await
            .expect("send reserved the account before reading the marker");

        assert_eq!(cache.load_pending_send(&account).await, Ok(None));
        assert_eq!(
            workflow
                .resolve_pending_send(MailSendOperationId::new())
                .await,
            Err(MailSendResolutionError::OperationInProgress)
        );
        assert_eq!(
            session.remove_account_cache().await,
            Err(AccountCacheRemovalError::OperationInProgress)
        );
        assert_eq!(send_calls.load(Ordering::Relaxed), 0);
        release_marker_read
            .send(())
            .await
            .expect("synthetic send marker read is released");
        assert_eq!(send_task.await, Ok(MailSendOutcome::Accepted));
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);
        assert_eq!(workflow.load_pending_send().await, Ok(None));
        let removal_result = session.remove_account_cache().await;
        assert_removal_completed(removal_result, &directory.path().join("mail.sqlite3"));
    });
}

#[test]
fn reply_reservation_blocks_account_removal_before_and_during_submission() {
    smol::block_on(async {
        let directory = TempDir::new().expect("synthetic account directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut provider = provider_with_cached_message(
            account.clone(),
            vec![cached_folder],
            started_at,
            Err(MailOperationError::ServiceUnavailable),
        );
        provider.send_result = Err(MailSendProviderError::OutcomeUnknown);
        let send_calls = Arc::clone(&provider.send_calls);
        let (submission_started, release_submission) = provider.hold_next_submission();
        let cache = open_cache(&directory, account.clone()).await;
        let session = open_session_over_retained_cache(provider, &account, &cache).await;
        let workflow = session.mail_session();
        workflow
            .synchronize_account(started_at, &ignore_sync_progress)
            .await
            .expect("synthetic reply source is cached");

        let (marker_read_started, release_marker_read) = cache.hold_pending_send_read_for_tests();
        let replying_workflow = Arc::clone(&workflow);
        let reply_source = MailReplySource::new(folder_reference, message_id());
        let reply_task = smol::spawn(async move {
            replying_workflow
                .reply_to_message(
                    MailReplyDraft::new(reply_source, "Synthetic reply body".to_owned()),
                    started_at,
                )
                .await
        });
        marker_read_started
            .recv()
            .await
            .expect("reply reserved the account before reading the marker");

        assert_eq!(cache.load_pending_send(&account).await, Ok(None));
        assert_eq!(
            session.remove_account_cache().await,
            Err(AccountCacheRemovalError::OperationInProgress)
        );
        release_marker_read
            .send(())
            .await
            .expect("synthetic reply marker read is released");
        submission_started
            .recv()
            .await
            .expect("reply reaches the provider with its account reservation held");
        assert_eq!(
            session.remove_account_cache().await,
            Err(AccountCacheRemovalError::OperationInProgress)
        );
        release_submission
            .send(())
            .await
            .expect("synthetic reply submission is released");
        assert!(matches!(
            reply_task.await,
            Ok(MailSendOutcome::OutcomeUnknown(_))
        ));
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            session.remove_account_cache().await,
            Err(AccountCacheRemovalError::PendingMailSend)
        );
        assert!(
            cache
                .load_pending_send(&account)
                .await
                .expect("unresolved reply marker remains readable")
                .is_some()
        );
        assert!(directory.path().join("mail.sqlite3").exists());
        drop(workflow);
        drop(session);
        cache.close_for_tests().await;
    });
}

#[test]
fn active_account_sync_blocks_cache_removal() {
    smol::block_on(async {
        let directory = TempDir::new().expect("synthetic account directory is created");
        let account = account_id();
        let mut provider = FixtureProvider::new(
            account.clone(),
            Vec::new(),
            [],
            Err(MailOperationError::ServiceUnavailable),
        );
        let (sync_started, sync_started_receiver) = smol::channel::bounded(1);
        let (release_sync, release_sync_receiver) = smol::channel::bounded(1);
        provider.folder_list_gate = Some((sync_started, release_sync_receiver));
        let cache = open_cache(&directory, account.clone()).await;
        let session = open_session_over_retained_cache(provider, &account, &cache).await;
        let workflow = session.mail_session();
        let syncing_workflow = Arc::clone(&workflow);
        let sync_task = smol::spawn(async move {
            syncing_workflow
                .synchronize_account(
                    UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                    &ignore_sync_progress,
                )
                .await
        });
        sync_started_receiver
            .recv()
            .await
            .expect("account sync owns its reservation while listing folders");

        assert_eq!(
            session.remove_account_cache().await,
            Err(AccountCacheRemovalError::OperationInProgress)
        );
        assert!(directory.path().join("mail.sqlite3").exists());
        release_sync
            .send(())
            .await
            .expect("synthetic folder listing gate is released");
        assert!(sync_task.await.is_ok());
        drop(workflow);
        drop(session);
        cache.close_for_tests().await;
    });
}

#[test]
fn active_folder_write_blocks_cache_removal() {
    smol::block_on(async {
        let directory = TempDir::new().expect("synthetic account directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let provider = provider_with_cached_message(
            account.clone(),
            vec![cached_folder],
            started_at,
            Err(MailOperationError::ServiceUnavailable),
        );
        let (write_started, release_write) = provider.hold_next_read_state();
        let cache = open_cache(&directory, account.clone()).await;
        let session = open_session_over_retained_cache(provider, &account, &cache).await;
        let workflow = session.mail_session();
        workflow
            .synchronize_account(started_at, &ignore_sync_progress)
            .await
            .expect("synthetic message is cached before its update");
        let writing_workflow = Arc::clone(&workflow);
        let writing_folder = folder_reference;
        let write_task = smol::spawn(async move {
            writing_workflow
                .set_message_read_state(&writing_folder, &message_id(), ReadState::Read)
                .await
        });
        write_started
            .recv()
            .await
            .expect("folder write owns its reservation while waiting for Graph");

        assert_eq!(
            session.remove_account_cache().await,
            Err(AccountCacheRemovalError::OperationInProgress)
        );
        release_write
            .send(())
            .await
            .expect("synthetic folder write is released");
        assert!(write_task.await.is_ok());
        drop(workflow);
        drop(session);
        cache.close_for_tests().await;
    });
}

#[test]
fn uncached_message_body_fetch_blocks_cache_removal() {
    smol::block_on(async {
        let directory = TempDir::new().expect("synthetic account directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let body = MessageBody::new(MessageBodyType::Text, "Synthetic body".to_owned());
        let mut provider = provider_with_cached_message(
            account.clone(),
            vec![cached_folder],
            started_at,
            Ok(body.clone()),
        );
        let body_fetches = Arc::clone(&provider.body_fetches);
        let (fetch_started_sender, fetch_started) = smol::channel::bounded(1);
        let (release_fetch, fetch_release_receiver) = smol::channel::bounded(1);
        provider.body_gate = Some(BodyFetchGate {
            started: fetch_started_sender,
            release: fetch_release_receiver,
        });
        let cache = open_cache(&directory, account.clone()).await;
        let session = open_session_over_retained_cache(provider, &account, &cache).await;
        let workflow = session.mail_session();
        workflow
            .synchronize_account(started_at, &ignore_sync_progress)
            .await
            .expect("synthetic message is cached before opening");
        let opening_workflow = Arc::clone(&workflow);
        let opening_folder = folder_reference.clone();
        let open_task = smol::spawn(async move {
            opening_workflow
                .open_message(&opening_folder, &message_id())
                .await
        });
        fetch_started
            .recv()
            .await
            .expect("message body request owns its folder reservation");

        assert_eq!(
            session.remove_account_cache().await,
            Err(AccountCacheRemovalError::OperationInProgress)
        );
        release_fetch
            .send(())
            .await
            .expect("synthetic body request is released");
        assert!(
            !open_task
                .await
                .expect("message opens after its body is fetched")
                .loaded_from_cache()
        );
        assert_eq!(body_fetches.load(Ordering::Relaxed), 1);
        assert_eq!(
            cache
                .read_message(&folder_reference, &message_id())
                .await
                .expect("cached message remains readable")
                .and_then(|(_, body)| body),
            Some(body)
        );
        let removal_result = session.remove_account_cache().await;
        assert_removal_completed(removal_result, &directory.path().join("mail.sqlite3"));
    });
}

#[test]
fn active_message_move_blocks_cache_removal() {
    smol::block_on(async {
        let directory = TempDir::new().expect("synthetic account directory is created");
        let account = account_id();
        let inbox = folder(&account);
        let archive = archive_folder(&account);
        let inbox_reference = inbox.reference().clone();
        let archive_reference = archive.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let cutoff = started_at - Duration::from_secs(90 * 24 * 60 * 60);
        let source_page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Synthetic subject".to_owned(), cutoff),
            }],
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-move-source"
                    .to_owned(),
            )),
        );
        let destination_page = MessageDeltaPage::new(
            Vec::new(),
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-move-destination"
                    .to_owned(),
            )),
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![inbox, archive],
            [
                Ok(page_response(source_page)),
                Ok(page_response(destination_page)),
            ],
            Err(MailOperationError::ServiceUnavailable),
        );
        let (move_started, release_move) = provider.hold_next_message_move();
        let cache = open_cache(&directory, account.clone()).await;
        let session = open_session_over_retained_cache(provider, &account, &cache).await;
        let workflow = session.mail_session();
        workflow
            .synchronize_account(started_at, &ignore_sync_progress)
            .await
            .expect("source and destination folders are cached");
        let moving_workflow = Arc::clone(&workflow);
        let move_task = smol::spawn(async move {
            moving_workflow
                .move_message(&inbox_reference, &message_id(), &archive_reference)
                .await
        });
        move_started
            .recv()
            .await
            .expect("message move holds its folder reservations during provider work");

        assert_eq!(
            session.remove_account_cache().await,
            Err(AccountCacheRemovalError::OperationInProgress)
        );
        release_move
            .send(())
            .await
            .expect("synthetic message move is released");
        assert_eq!(
            move_task.await,
            Ok(einfach_mail_application::MessageMoveOutcome::ConfirmedAndCached)
        );
        let removal_result = session.remove_account_cache().await;
        assert_removal_completed(removal_result, &directory.path().join("mail.sqlite3"));
    });
}
