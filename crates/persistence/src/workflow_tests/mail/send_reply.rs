#[test]
fn unknown_send_marker_survives_encrypted_restart_and_blocks_duplicate_send_and_removal() {
    smol::block_on(async {
        let directory = TempDir::new().expect("synthetic mail cache directory is created");
        let account = account_id();
        let provider = FixtureProvider::new(
            account.clone(),
            Vec::new(),
            [],
            Err(MailOperationError::ServiceUnavailable),
        )
        .with_send_result(Err(MailSendProviderError::OutcomeUnknown));
        let send_calls = Arc::clone(&provider.send_calls);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(account.clone(), provider.clone(), cache.clone());
        let result = workflow
            .send_message(
                draft("Synthetic subject", "synthetic-private-body"),
                UNIX_EPOCH + Duration::from_secs(1_800_000_000),
            )
            .await
            .expect("ambiguous submission is represented as an outcome");
        let pending = match result {
            MailSendOutcome::OutcomeUnknown(pending) => pending,
            other => panic!("expected unresolved send marker, got {other:?}"),
        };
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            workflow.load_pending_send().await,
            Ok(Some(pending.clone()))
        );
        drop(workflow);
        cache.close_for_tests().await;
        drop(cache);

        let encrypted_bytes = std::fs::read(directory.path().join("mail.sqlite3"))
            .expect("SQLCipher database remains on disk");
        assert!(
            !encrypted_bytes
                .windows(b"synthetic-private-body".len())
                .any(|window| window == b"synthetic-private-body")
        );

        let reopened = open_cache(&directory, account.clone()).await;
        assert_eq!(
            reopened.load_pending_send(&account).await,
            Ok(Some(pending.clone()))
        );
        let workflow = MailWorkflow::new(account.clone(), provider, reopened.clone());
        let duplicate = workflow
            .send_message(
                draft("Another synthetic subject", "another-body"),
                UNIX_EPOCH + Duration::from_secs(1_800_000_001),
            )
            .await;
        assert!(matches!(
            duplicate,
            Err(MailSendWorkflowError::PendingResolutionRequired(ref saved))
                if saved == &pending
        ));
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            <SqliteAccountCache as AccountCache>::remove_account(&reopened, &account).await,
            Err(AccountCacheRemovalError::PendingMailSend)
        );

        workflow
            .resolve_pending_send(pending.operation_id())
            .await
            .expect("explicit resolution clears the old marker without resending");
        assert_eq!(workflow.load_pending_send().await, Ok(None));
        drop(workflow);
        reopened.close_for_tests().await;
    });
}

#[test]
fn unknown_reply_marker_survives_encrypted_restart_without_reply_body() {
    smol::block_on(async {
        let directory = TempDir::new().expect("synthetic cache directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let cutoff = started_at - Duration::from_secs(90 * 24 * 60 * 60);
        let delta_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-reply".to_owned(),
        );
        let page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Synthetic source subject".to_owned(), cutoff),
            }],
            MessageDeltaProgress::Complete(delta_link),
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            [Ok(page_response(page))],
            Err(MailOperationError::ServiceUnavailable),
        )
        .with_send_result(Err(MailSendProviderError::OutcomeUnknown));
        let send_calls = Arc::clone(&provider.send_calls);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(account.clone(), provider.clone(), cache.clone());
        workflow
            .synchronize_account(started_at, &ignore_sync_progress)
            .await
            .expect("synthetic reply source is cached");
        let source = MailReplySource::new(folder_reference.clone(), message_id());
        let reply_body = "Synthetic private reply body";
        let result = workflow
            .reply_to_message(
                MailReplyDraft::new(source.clone(), reply_body.to_owned()),
                started_at,
            )
            .await
            .expect("unknown reply result is retained for explicit resolution");
        let pending = match result {
            MailSendOutcome::OutcomeUnknown(pending) => pending,
            other => panic!("expected unresolved reply marker, got {other:?}"),
        };
        assert_eq!(pending.target().subject(), Some("Synthetic source subject"));
        assert_eq!(pending.target().reply_source(), Some(&source));
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            workflow
                .reply_to_message(
                    MailReplyDraft::new(source.clone(), reply_body.to_owned()),
                    started_at,
                )
                .await,
            Err(MailSendWorkflowError::PendingResolutionRequired(
                pending.clone()
            ))
        );
        assert_eq!(
            workflow
                .send_message(draft("Another subject", "another body"), started_at)
                .await,
            Err(MailSendWorkflowError::PendingResolutionRequired(
                pending.clone()
            ))
        );
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            cache.load_pending_send(&account).await,
            Ok(Some(pending.clone()))
        );
        assert_eq!(
            <SqliteAccountCache as AccountCache>::remove_account(&cache, &account).await,
            Err(AccountCacheRemovalError::PendingMailSend)
        );

        drop(workflow);
        cache.close_for_tests().await;
        let encrypted_bytes = std::fs::read(directory.path().join("mail.sqlite3"))
            .expect("encrypted cache remains on disk");
        assert!(
            !encrypted_bytes
                .windows(reply_body.len())
                .any(|window| window == reply_body.as_bytes())
        );

        let reopened = open_cache(&directory, account.clone()).await;
        assert_eq!(
            reopened.load_pending_send(&account).await,
            Ok(Some(pending.clone()))
        );
        let workflow = MailWorkflow::new(account.clone(), provider, reopened.clone());
        assert_eq!(
            workflow
                .reply_to_message(
                    MailReplyDraft::new(source, reply_body.to_owned()),
                    started_at,
                )
                .await,
            Err(MailSendWorkflowError::PendingResolutionRequired(
                pending.clone()
            ))
        );
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);
        workflow
            .resolve_pending_send(pending.operation_id())
            .await
            .expect("explicit Sent Items reconciliation clears the marker");
        assert_eq!(workflow.load_pending_send().await, Ok(None));
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);
        drop(workflow);
        reopened.close_for_tests().await;
    });
}

#[test]
fn reply_requires_a_cached_source_owned_by_the_requested_account() {
    smol::block_on(async {
        let directory = TempDir::new().expect("synthetic cache directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let delta_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-empty".to_owned(),
        );
        let empty_page =
            MessageDeltaPage::new(Vec::new(), MessageDeltaProgress::Complete(delta_link));
        let provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            [Ok(page_response(empty_page))],
            Err(MailOperationError::ServiceUnavailable),
        );
        let send_calls = Arc::clone(&provider.send_calls);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(account.clone(), provider, cache.clone());
        workflow
            .synchronize_account(started_at, &ignore_sync_progress)
            .await
            .expect("the empty synthetic folder is cached");

        let other_account = AccountId::try_from("synthetic-other-account".to_owned())
            .expect("synthetic account ID is valid");
        let other_folder = folder(&other_account).reference().clone();
        assert_eq!(
            workflow
                .reply_to_message(
                    MailReplyDraft::new(
                        MailReplySource::new(other_folder, message_id()),
                        "Synthetic reply body".to_owned(),
                    ),
                    started_at,
                )
                .await,
            Err(MailSendWorkflowError::Provider(
                MailSendProviderError::AccountMismatch
            ))
        );
        assert_eq!(
            workflow
                .reply_to_message(
                    MailReplyDraft::new(
                        MailReplySource::new(folder_reference, message_id()),
                        "Synthetic reply body".to_owned(),
                    ),
                    started_at,
                )
                .await,
            Err(MailSendWorkflowError::ReplySourceNotCached)
        );
        assert_eq!(send_calls.load(Ordering::Relaxed), 0);
        assert_eq!(cache.load_pending_send(&account).await, Ok(None));
        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn accepted_and_definitively_rejected_sends_clear_their_markers() {
    smol::block_on(async {
        let directory = TempDir::new().expect("synthetic mail cache directory is created");
        let account = account_id();
        let cache = open_cache(&directory, account.clone()).await;
        let accepted_provider = FixtureProvider::new(
            account.clone(),
            Vec::new(),
            [],
            Err(MailOperationError::ServiceUnavailable),
        )
        .with_send_result(Ok(()));
        let accepted_calls = Arc::clone(&accepted_provider.send_calls);
        let accepted_workflow =
            MailWorkflow::new(account.clone(), accepted_provider, cache.clone());

        assert_eq!(
            accepted_workflow
                .send_message(
                    draft("Accepted synthetic subject", "accepted synthetic body"),
                    UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                )
                .await,
            Ok(MailSendOutcome::Accepted)
        );
        assert_eq!(accepted_workflow.load_pending_send().await, Ok(None));
        assert_eq!(accepted_calls.load(Ordering::Relaxed), 1);
        drop(accepted_workflow);

        let rejected_provider = FixtureProvider::new(
            account.clone(),
            Vec::new(),
            [],
            Err(MailOperationError::ServiceUnavailable),
        )
        .with_send_result(Err(MailSendProviderError::Rejected));
        let rejected_calls = Arc::clone(&rejected_provider.send_calls);
        let rejected_workflow =
            MailWorkflow::new(account.clone(), rejected_provider, cache.clone());

        assert_eq!(
            rejected_workflow
                .send_message(
                    draft("Rejected synthetic subject", "rejected synthetic body"),
                    UNIX_EPOCH + Duration::from_secs(1_800_000_001),
                )
                .await,
            Err(MailSendWorkflowError::Provider(
                MailSendProviderError::Rejected
            ))
        );
        assert_eq!(rejected_workflow.load_pending_send().await, Ok(None));
        assert_eq!(rejected_calls.load(Ordering::Relaxed), 1);
        drop(rejected_workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn rejected_send_with_marker_clear_failure_keeps_a_resolvable_known_rejection() {
    smol::block_on(async {
        let directory = TempDir::new().expect("synthetic mail cache directory is created");
        let account = account_id();
        let cache = open_cache(&directory, account.clone()).await;
        cache.fail_next_pending_send_clear();
        let provider = FixtureProvider::new(
            account.clone(),
            Vec::new(),
            [],
            Err(MailOperationError::ServiceUnavailable),
        )
        .with_send_result(Err(MailSendProviderError::Rejected));
        let send_calls = Arc::clone(&provider.send_calls);
        let workflow = MailWorkflow::new(account.clone(), provider, cache.clone());

        let pending = match workflow
            .send_message(
                draft("Rejected synthetic subject", "rejected synthetic body"),
                UNIX_EPOCH + Duration::from_secs(1_800_000_000),
            )
            .await
        {
            Err(MailSendWorkflowError::RejectedNeedsResolution {
                pending,
                provider: MailSendProviderError::Rejected,
                cache: MailCacheError::Unavailable,
            }) => pending,
            other => panic!("expected the known rejection and its retained marker, got {other:?}"),
        };

        assert_eq!(
            workflow.load_pending_send().await,
            Ok(Some((*pending).clone()))
        );
        workflow
            .resolve_pending_send(pending.operation_id())
            .await
            .expect("explicit recovery clears the known rejection without resending");
        assert_eq!(workflow.load_pending_send().await, Ok(None));
        assert_eq!(send_calls.load(Ordering::Relaxed), 1);

        drop(workflow);
        cache.close_for_tests().await;
    });
}
