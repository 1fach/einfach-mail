use super::*;

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
        let event = versioned_calendar_event(view);

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
                    MailFolderKind::Archive,
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
