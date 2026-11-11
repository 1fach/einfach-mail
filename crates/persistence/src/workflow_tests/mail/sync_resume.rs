use super::*;

#[test]
fn interrupted_sync_resumes_its_checkpoint_and_body_reopens_from_encrypted_cache() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let cutoff = started_at - Duration::from_secs(90 * 24 * 60 * 60);
        let next_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/messages?$skiptoken=synthetic-resume".to_owned(),
        );
        let final_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-final".to_owned(),
        );
        let initial_provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder.clone()],
            [
                Ok(page_response(first_page(cutoff, next_link.clone()))),
                Err(MailOperationError::ServiceUnavailable),
            ],
            Ok(MessageBody::new(
                MessageBodyType::Html,
                "<p onclick=\"run()\">Synthetic body</p><script>private marker</script>".to_owned(),
            )),
        );
        let initial_requests = Arc::clone(&initial_provider.requests);
        let initial_body_fetches = Arc::clone(&initial_provider.body_fetches);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(account.clone(), initial_provider, cache.clone());

        assert_eq!(
            workflow
                .synchronize_account(started_at, &ignore_sync_progress)
                .await,
            Err(MailWorkflowError::Provider(
                MailOperationError::ServiceUnavailable
            ))
        );
        let saved_state = workflow
            .cached_sync_state(&folder_reference)
            .await
            .expect("interrupted sync state is readable")
            .expect("initial cutoff was saved before the request");
        assert_eq!(saved_state.cutoff(), cutoff);
        assert_eq!(saved_state.next_link(), Some(&next_link));
        assert!(saved_state.delta_link().is_none());
        assert_eq!(
            workflow
                .list_cached_messages(&folder_reference)
                .await
                .expect("page metadata is cached before interruption")
                .len(),
            1
        );
        assert_eq!(
            initial_requests
                .lock()
                .expect("synthetic request log mutex is not poisoned")
                .as_slice(),
            [
                MessageDeltaRequest::Initial,
                MessageDeltaRequest::Continue(next_link.clone()),
            ]
        );
        assert_eq!(initial_body_fetches.load(Ordering::Relaxed), 0);
        drop(workflow);

        let resume_provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            [Ok(page_response(final_page(final_link.clone())))],
            Ok(MessageBody::new(
                MessageBodyType::Html,
                "<p onclick=\"run()\">Synthetic body</p><script>private marker</script>".to_owned(),
            )),
        );
        let resume_requests = Arc::clone(&resume_provider.requests);
        let body_fetches = Arc::clone(&resume_provider.body_fetches);
        let resumed_workflow = MailWorkflow::new(account.clone(), resume_provider, cache.clone());
        let report = resumed_workflow
            .synchronize_account(
                started_at + Duration::from_secs(86_400),
                &ignore_sync_progress,
            )
            .await
            .expect("the interrupted folder round resumes");
        assert_eq!(report.folder_count(), 1);
        assert_eq!(report.page_count(), 1);
        assert_eq!(
            resume_requests
                .lock()
                .expect("synthetic request log mutex is not poisoned")
                .as_slice(),
            [MessageDeltaRequest::Continue(next_link)]
        );
        let complete_state = resumed_workflow
            .cached_sync_state(&folder_reference)
            .await
            .expect("completed cursor is readable")
            .expect("folder cursor remains present");
        assert_eq!(complete_state.cutoff(), cutoff);
        assert_eq!(complete_state.delta_link(), Some(&final_link));
        assert!(complete_state.next_link().is_none());
        assert!(complete_state.last_synchronized_at().is_some());

        let opened = resumed_workflow
            .open_message(&folder_reference, &message_id())
            .await
            .expect("opening a message fetches and caches its body");
        assert!(!opened.loaded_from_cache());
        assert!(matches!(
            opened.body(),
            MessageBodyForDisplay::SanitizedHtml(html)
                if html.as_str() == "<p>Synthetic body</p>"
        ));
        let (_, cached_message) = cache
            .read_message(&folder_reference, &message_id())
            .await
            .expect("raw cached body can be read")
            .expect("message remains cached");
        assert_eq!(
            cached_message.expect("body was persisted").content(),
            "<p onclick=\"run()\">Synthetic body</p><script>private marker</script>"
        );
        assert_eq!(body_fetches.load(Ordering::Relaxed), 1);
        drop(resumed_workflow);
        cache.close_for_tests().await;

        let reopened_cache = open_cache(&directory, account.clone()).await;
        let offline_provider = FixtureProvider::new(
            account.clone(),
            vec![folder(&account_id())],
            [],
            Err(MailOperationError::ServiceUnavailable),
        );
        let offline_fetches = Arc::clone(&offline_provider.body_fetches);
        let offline_workflow =
            MailWorkflow::new(account.clone(), offline_provider, reopened_cache.clone());
        let reopened_message = offline_workflow
            .open_message(&folder_reference, &message_id())
            .await
            .expect("cached body opens without a provider request");
        assert!(reopened_message.loaded_from_cache());
        assert!(matches!(
            reopened_message.body(),
            MessageBodyForDisplay::SanitizedHtml(html)
                if html.as_str() == "<p>Synthetic body</p>"
        ));
        let (_, reopened_cached_body) = reopened_cache
            .read_message(&folder_reference, &message_id())
            .await
            .expect("raw cached body can be read after restart")
            .expect("message remains cached after restart");
        assert_eq!(
            reopened_cached_body
                .expect("raw body remains in the encrypted cache")
                .content(),
            "<p onclick=\"run()\">Synthetic body</p><script>private marker</script>"
        );
        assert_eq!(offline_fetches.load(Ordering::Relaxed), 0);
        drop(offline_workflow);
        reopened_cache.close_for_tests().await;
    });
}

#[test]
fn cancelling_sync_with_a_pending_continuation_releases_reservation_and_resumes_checkpoint() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let cutoff = started_at - Duration::from_secs(90 * 24 * 60 * 60);
        let next_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/messages?$skiptoken=synthetic-pending".to_owned(),
        );
        let final_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-final".to_owned(),
        );
        let mut provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            [
                Ok(page_response(first_page(cutoff, next_link.clone()))),
                Ok(page_response(final_page(final_link.clone()))),
            ],
            Err(MailOperationError::ServiceUnavailable),
        );
        let requests = Arc::clone(&provider.requests);
        let (started_sender, started_receiver) = smol::channel::bounded(1);
        let (_release_sender, release_receiver) = smol::channel::bounded(1);
        provider.delta_gate = Some(DeltaPageGate {
            request_count: Arc::new(AtomicUsize::new(0)),
            request_index_to_block: 1,
            started: started_sender,
            release: release_receiver,
        });
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = Arc::new(MailWorkflow::new(account, provider, cache.clone()));
        let executor = smol::Executor::new();
        let (report, state) = smol::block_on(executor.run(async {
            let syncing_workflow = Arc::clone(&workflow);
            let sync_task = executor.spawn(async move {
                syncing_workflow
                    .synchronize_account(started_at, &ignore_sync_progress)
                    .await
            });
            started_receiver
                .recv()
                .await
                .expect("the continuation request reaches its gate");

            let checkpoint = workflow
                .cached_sync_state(&folder_reference)
                .await
                .expect("the first page checkpoint is readable")
                .expect("the initial cutoff is stored");
            assert_eq!(checkpoint.cutoff(), cutoff);
            assert_eq!(checkpoint.next_link(), Some(&next_link));
            assert_eq!(
                workflow
                    .list_cached_messages(&folder_reference)
                    .await
                    .expect("the first page remains cached")
                    .len(),
                1
            );

            assert!(sync_task.cancel().await.is_none());

            let report = workflow
                .synchronize_account(
                    started_at + Duration::from_secs(86_400),
                    &ignore_sync_progress,
                )
                .await
                .expect("a later sync acquires the released reservation");
            let state = workflow
                .cached_sync_state(&folder_reference)
                .await
                .expect("the completed cursor is readable")
                .expect("the folder cursor remains present");
            (report, state)
        }));

        assert_eq!(report.page_count(), 1);
        assert_eq!(state.cutoff(), cutoff);
        assert_eq!(state.delta_link(), Some(&final_link));
        assert!(state.next_link().is_none());
        assert_eq!(
            requests
                .lock()
                .expect("synthetic request log mutex is not poisoned")
                .as_slice(),
            [
                MessageDeltaRequest::Initial,
                MessageDeltaRequest::Continue(next_link.clone()),
                MessageDeltaRequest::Continue(next_link),
            ]
        );

        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn opening_a_missing_body_reserves_its_folder_against_delta_sync() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let old_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/messages?$deltatoken=before-race".to_owned(),
        );
        let new_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/messages?$deltatoken=after-race".to_owned(),
        );
        let initial_page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Before sync".to_owned(), started_at),
            }],
            MessageDeltaProgress::Complete(old_link),
        );
        let cache = open_cache(&directory, account.clone()).await;
        let initial_workflow = MailWorkflow::new(
            account.clone(),
            FixtureProvider::new(
                account.clone(),
                vec![cached_folder.clone()],
                [Ok(page_response(initial_page))],
                Err(MailOperationError::ServiceUnavailable),
            ),
            cache.clone(),
        );
        initial_workflow
            .synchronize_account(started_at, &ignore_sync_progress)
            .await
            .expect("initial metadata sync completes");

        let update_page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("After sync".to_owned(), started_at),
            }],
            MessageDeltaProgress::Complete(new_link),
        );
        let mut provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            [Ok(page_response(update_page))],
            Ok(MessageBody::new(
                MessageBodyType::Text,
                "Synthetic fetched body".to_owned(),
            )),
        );
        let (started_sender, started_receiver) = smol::channel::unbounded();
        let (release_sender, release_receiver) = smol::channel::bounded(1);
        provider.body_gate = Some(BodyFetchGate {
            started: started_sender,
            release: release_receiver,
        });
        let workflow = Arc::new(MailWorkflow::new(account.clone(), provider, cache.clone()));
        let executor = smol::Executor::new();
        let open_workflow = Arc::clone(&workflow);
        let open_folder = folder_reference.clone();
        let open_message_id = message_id();

        let (open_result, sync_result) = smol::block_on(executor.run(async {
            let open_task = executor.spawn(async move {
                open_workflow
                    .open_message(&open_folder, &open_message_id)
                    .await
            });
            started_receiver
                .recv()
                .await
                .expect("body fetch reaches the synchronization gate");
            let sync_result = workflow
                .synchronize_account(started_at + Duration::from_secs(1), &ignore_sync_progress)
                .await;
            release_sender
                .send(())
                .await
                .expect("body fetch gate is still open");
            (open_task.await, sync_result)
        }));

        let opened = open_result.expect("message opens after the body fetch is released");
        assert!(!opened.loaded_from_cache());
        assert_eq!(
            sync_result,
            Err(MailWorkflowError::OperationInProgress),
            "a delta page cannot commit while a missing body is being fetched"
        );
        let (_, cached_body) = cache
            .read_message(&folder_reference, &message_id())
            .await
            .expect("message remains readable after the first open")
            .expect("message metadata remains cached");
        assert_eq!(
            cached_body.expect("fetched body is cached").content(),
            "Synthetic fetched body"
        );

        workflow
            .synchronize_account(started_at + Duration::from_secs(2), &ignore_sync_progress)
            .await
            .expect("delta sync completes after the open releases its reservation");
        let (updated_message, cached_body) = cache
            .read_message(&folder_reference, &message_id())
            .await
            .expect("updated message remains readable")
            .expect("updated message remains cached");
        assert_eq!(updated_message.metadata().subject(), Some("After sync"));
        assert!(cached_body.is_none());

        drop(workflow);
        drop(initial_workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn sync_keeps_all_matching_messages_across_the_five_thousand_message_boundary() {
    smol::block_on(async {
        const FIRST_PAGE_COUNT: usize = 5_000;

        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let cutoff = started_at - Duration::from_secs(90 * 24 * 60 * 60);
        let next_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/messages?$skiptoken=synthetic-5001".to_owned(),
        );
        let final_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-complete".to_owned(),
        );
        let first_changes = (0..FIRST_PAGE_COUNT)
            .map(|index| MessageDeltaChange::Upsert {
                id: MessageId::try_from(format!("synthetic-message-{index:04}"))
                    .expect("synthetic message ID is valid"),
                patch: complete_patch(format!("Synthetic message {index}"), started_at),
            })
            .collect();
        let final_changes = vec![
            MessageDeltaChange::Upsert {
                id: MessageId::try_from("synthetic-message-5000".to_owned())
                    .expect("synthetic message ID is valid"),
                patch: complete_patch("Synthetic message 5000".to_owned(), started_at),
            },
            MessageDeltaChange::Upsert {
                id: MessageId::try_from("synthetic-outside-cutoff".to_owned())
                    .expect("synthetic message ID is valid"),
                patch: complete_patch(
                    "Outside cutoff".to_owned(),
                    cutoff - Duration::from_nanos(1),
                ),
            },
        ];
        let provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            [
                Ok(page_response(MessageDeltaPage::new(
                    first_changes,
                    MessageDeltaProgress::Next(next_link.clone()),
                ))),
                Ok(page_response(MessageDeltaPage::new(
                    final_changes,
                    MessageDeltaProgress::Complete(final_link.clone()),
                ))),
            ],
            Err(MailOperationError::ServiceUnavailable),
        );
        let requests = Arc::clone(&provider.requests);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(account.clone(), provider, cache.clone());

        let report = workflow
            .synchronize_account(started_at, &ignore_sync_progress)
            .await
            .expect("both pages commit before the final cursor advances");
        let messages = workflow
            .list_cached_messages(&folder_reference)
            .await
            .expect("all in-scope message metadata is readable");
        let state = workflow
            .cached_sync_state(&folder_reference)
            .await
            .expect("the folder cursor remains readable")
            .expect("the folder cursor was initialized");

        assert_eq!(report.folder_count(), 1);
        assert_eq!(report.page_count(), 2);
        assert_eq!(messages.len(), FIRST_PAGE_COUNT + 1);
        assert!(
            messages
                .iter()
                .all(|message| { message.metadata().received_at() >= &cutoff })
        );
        assert_eq!(state.delta_link(), Some(&final_link));
        assert!(state.next_link().is_none());
        assert_eq!(
            requests
                .lock()
                .expect("synthetic request log mutex is not poisoned")
                .as_slice(),
            [
                MessageDeltaRequest::Initial,
                MessageDeltaRequest::Continue(next_link)
            ]
        );
        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn response_budget_stops_before_committing_an_over_budget_page() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let pages = (0..9).map(|page_index| {
            let next_link = MessageDeltaLink::from_opaque(format!(
                "https://graph.microsoft.com/v1.0/messages?$skiptoken=synthetic-page-{page_index}"
            ));
            Ok(MessageDeltaResponse::Page(MessageDeltaPageResponse::new(
                MessageDeltaPage::new(Vec::new(), MessageDeltaProgress::Next(next_link)),
                8 * 1024 * 1024,
            )))
        });
        let provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            pages,
            Err(MailOperationError::ServiceUnavailable),
        );
        let requests = Arc::clone(&provider.requests);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(account.clone(), provider, cache.clone());

        let error = workflow
            .synchronize_account(
                UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                &ignore_sync_progress,
            )
            .await
            .expect_err("response budget prevents unbounded delta traversal");
        assert_eq!(error, MailWorkflowError::ResponseLimitExceeded);

        let state = workflow
            .cached_sync_state(&folder_reference)
            .await
            .expect("last committed checkpoint remains readable")
            .expect("folder sync state exists");
        assert_eq!(
            state.next_link().map(MessageDeltaLink::as_str),
            Some("https://graph.microsoft.com/v1.0/messages?$skiptoken=synthetic-page-7")
        );
        assert_eq!(
            requests.lock().expect("request log mutex is healthy").len(),
            9
        );
        assert!(
            workflow
                .list_cached_messages(&folder_reference)
                .await
                .expect("cached messages remain readable")
                .is_empty()
        );
        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn mail_page_limit_counts_pages_across_a_cursor_reset() {
    smol::block_on(async {
        const PAGE_LIMIT: usize = 1_000;

        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let old_delta_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken=old"
                .to_owned(),
        );
        let cache = open_cache(&directory, account.clone()).await;
        let baseline_workflow = MailWorkflow::new(
            account.clone(),
            FixtureProvider::new(
                account.clone(),
                vec![cached_folder.clone()],
                [Ok(page_response(MessageDeltaPage::new(
                    Vec::new(),
                    MessageDeltaProgress::Complete(old_delta_link.clone()),
                )))],
                Err(MailOperationError::ServiceUnavailable),
            ),
            cache.clone(),
        );
        baseline_workflow
            .synchronize_account(started_at, &ignore_sync_progress)
            .await
            .expect("baseline delta cursor is saved");
        drop(baseline_workflow);

        let reset_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken="
                .to_owned(),
        );
        let replacement_next_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$skiptoken=replacement-resume"
                .to_owned(),
        );
        let replacement_delta_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken=replacement-complete"
                .to_owned(),
        );
        let mut pages = Vec::with_capacity(PAGE_LIMIT + 2);
        for page_index in 0..PAGE_LIMIT - 1 {
            let next_link = MessageDeltaLink::from_opaque(format!(
                "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$skiptoken=before-reset-{page_index}"
            ));
            pages.push(Ok(page_response(MessageDeltaPage::new(
                Vec::new(),
                MessageDeltaProgress::Next(next_link),
            ))));
        }
        pages.push(Ok(MessageDeltaResponse::Reset(reset_link)));
        pages.push(Ok(page_response(MessageDeltaPage::new(
            Vec::new(),
            MessageDeltaProgress::Next(replacement_next_link.clone()),
        ))));
        pages.push(Ok(page_response(MessageDeltaPage::new(
            Vec::new(),
            MessageDeltaProgress::Complete(replacement_delta_link.clone()),
        ))));
        let provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            pages,
            Err(MailOperationError::ServiceUnavailable),
        );
        let requests = Arc::clone(&provider.requests);
        let workflow = MailWorkflow::new(account.clone(), provider, cache.clone());

        let error = workflow
            .synchronize_account(started_at + Duration::from_secs(1), &ignore_sync_progress)
            .await
            .expect_err("the attempt stops at its aggregate page limit");
        assert_eq!(error, MailWorkflowError::PageLimitExceeded);
        assert_eq!(
            requests
                .lock()
                .expect("synthetic request log mutex is healthy")
                .len(),
            PAGE_LIMIT + 1
        );
        let interrupted_state = workflow
            .cached_sync_state(&folder_reference)
            .await
            .expect("replacement checkpoint remains readable")
            .expect("folder state exists");
        assert_eq!(interrupted_state.delta_link(), Some(&old_delta_link));
        assert_eq!(interrupted_state.next_link(), Some(&replacement_next_link));
        assert!(interrupted_state.is_rebuilding());

        let report = workflow
            .synchronize_account(started_at + Duration::from_secs(2), &ignore_sync_progress)
            .await
            .expect("the next attempt resumes the replacement scan");
        assert_eq!(report.page_count(), 1);
        let completed_state = workflow
            .cached_sync_state(&folder_reference)
            .await
            .expect("replacement cursor remains readable")
            .expect("folder state exists");
        assert_eq!(completed_state.delta_link(), Some(&replacement_delta_link));
        assert!(completed_state.next_link().is_none());
        assert!(!completed_state.is_rebuilding());
        assert_eq!(
            requests
                .lock()
                .expect("synthetic request log mutex is healthy")
                .last(),
            Some(&MessageDeltaRequest::Continue(replacement_next_link))
        );
        drop(workflow);
        cache.close_for_tests().await;
    });
}
