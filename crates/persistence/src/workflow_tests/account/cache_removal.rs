use super::*;

#[test]
fn account_cache_removal_preserves_the_in_use_error() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let first = open_cache(&directory, account.clone()).await;
        let second = open_cache(&directory, account.clone()).await;

        assert_eq!(
            <SqliteAccountCache as AccountCache>::remove_account(&first, &account).await,
            Err(AccountCacheRemovalError::InUse)
        );

        first.close_for_tests().await;
        second.close_for_tests().await;
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
