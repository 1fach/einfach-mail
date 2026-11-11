use super::*;

#[test]
fn expired_delta_cursor_rebuild_keeps_cached_mail_until_the_snapshot_commits() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let stale_id =
            MessageId::try_from("stale-message".to_owned()).expect("synthetic message ID is valid");
        let temporary_id = MessageId::try_from("temporary-message".to_owned())
            .expect("synthetic message ID is valid");
        let old_delta_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken=old"
                .to_owned(),
        );
        let reset_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken="
                .to_owned(),
        );
        let next_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$skiptoken=resume"
                .to_owned(),
        );
        let new_delta_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken=new"
                .to_owned(),
        );
        let baseline_page = MessageDeltaPage::new(
            vec![
                MessageDeltaChange::Upsert {
                    id: message_id(),
                    patch: complete_patch("Cached message".to_owned(), started_at),
                },
                MessageDeltaChange::Upsert {
                    id: stale_id.clone(),
                    patch: complete_patch("Stale message".to_owned(), started_at),
                },
            ],
            MessageDeltaProgress::Complete(old_delta_link.clone()),
        );
        let cache = open_cache(&directory, account.clone()).await;
        let baseline_workflow = MailWorkflow::new(
            account.clone(),
            FixtureProvider::new(
                account.clone(),
                vec![cached_folder.clone()],
                [Ok(page_response(baseline_page))],
                Err(MailOperationError::ServiceUnavailable),
            ),
            cache.clone(),
        );
        baseline_workflow
            .synchronize_account(started_at, &ignore_sync_progress)
            .await
            .expect("baseline snapshot completes");
        cache
            .store_message_body(
                &folder_reference,
                &message_id(),
                MessageBody::new(MessageBodyType::Text, "Synthetic cached body".to_owned()),
            )
            .await
            .expect("body is cached before replacement starts");
        cache
            .store_message_body(
                &folder_reference,
                &stale_id,
                MessageBody::new(MessageBodyType::Text, "Synthetic stale body".to_owned()),
            )
            .await
            .expect("unseen body is cached before replacement starts");
        drop(baseline_workflow);
        cache.close_for_tests().await;

        let cache = open_cache(&directory, account.clone()).await;
        let replacement_page = MessageDeltaPage::new(
            vec![
                MessageDeltaChange::Upsert {
                    id: message_id(),
                    patch: complete_patch("Updated message".to_owned(), started_at),
                },
                MessageDeltaChange::Upsert {
                    id: temporary_id.clone(),
                    patch: complete_patch("Temporary message".to_owned(), started_at),
                },
            ],
            MessageDeltaProgress::Next(next_link.clone()),
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder.clone()],
            [
                Ok(MessageDeltaResponse::Reset(reset_link.clone())),
                Ok(page_response(replacement_page)),
                Err(MailOperationError::ServiceUnavailable),
            ],
            Err(MailOperationError::ServiceUnavailable),
        );
        let requests = Arc::clone(&provider.requests);
        let workflow = MailWorkflow::new(account.clone(), provider, cache.clone());

        assert_eq!(
            workflow
                .synchronize_account(started_at + Duration::from_secs(1), &ignore_sync_progress,)
                .await,
            Err(MailWorkflowError::Provider(
                MailOperationError::ServiceUnavailable
            ))
        );
        let interrupted_state = workflow
            .cached_sync_state(&folder_reference)
            .await
            .expect("replacement checkpoint remains readable")
            .expect("folder state remains present");
        assert_eq!(interrupted_state.delta_link(), Some(&old_delta_link));
        assert_eq!(interrupted_state.next_link(), Some(&next_link));
        assert_eq!(interrupted_state.full_sync_generation(), Some(1));
        assert!(interrupted_state.is_rebuilding());
        let visible_before_resume = workflow
            .list_cached_messages(&folder_reference)
            .await
            .expect("old and replacement messages remain visible");
        assert_eq!(visible_before_resume.len(), 3);
        assert!(
            visible_before_resume
                .iter()
                .any(|message| message.id() == &stale_id)
        );
        let (_, cached_body) = cache
            .read_message(&folder_reference, &message_id())
            .await
            .expect("matching message remains readable")
            .expect("matching message remains cached");
        assert!(
            cached_body.is_none(),
            "updated metadata invalidates its body"
        );
        let (_, stale_body) = cache
            .read_message(&folder_reference, &stale_id)
            .await
            .expect("unseen message remains readable during replacement")
            .expect("unseen message remains cached during replacement");
        assert_eq!(
            stale_body
                .expect("unseen body remains until the scan completes")
                .content(),
            "Synthetic stale body"
        );
        assert_eq!(
            requests
                .lock()
                .expect("synthetic request log mutex is healthy")
                .as_slice(),
            [
                MessageDeltaRequest::Continue(old_delta_link),
                MessageDeltaRequest::Continue(reset_link),
                MessageDeltaRequest::Continue(next_link.clone()),
            ]
        );
        drop(workflow);
        cache.close_for_tests().await;

        let cache = open_cache(&directory, account.clone()).await;
        let final_page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Removed { id: temporary_id }],
            MessageDeltaProgress::Complete(new_delta_link.clone()),
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            [Ok(page_response(final_page))],
            Err(MailOperationError::ServiceUnavailable),
        );
        let requests = Arc::clone(&provider.requests);
        let workflow = MailWorkflow::new(account.clone(), provider, cache.clone());
        workflow
            .synchronize_account(started_at + Duration::from_secs(2), &ignore_sync_progress)
            .await
            .expect("replacement scan resumes from its saved page link");

        let completed_state = workflow
            .cached_sync_state(&folder_reference)
            .await
            .expect("replacement state is readable")
            .expect("folder state remains present");
        assert_eq!(completed_state.delta_link(), Some(&new_delta_link));
        assert!(completed_state.next_link().is_none());
        assert!(!completed_state.is_rebuilding());
        let visible_after_resume = workflow
            .list_cached_messages(&folder_reference)
            .await
            .expect("completed snapshot is readable");
        assert_eq!(visible_after_resume.len(), 1);
        assert_eq!(visible_after_resume[0].id(), &message_id());
        let (_, cached_body) = cache
            .read_message(&folder_reference, &message_id())
            .await
            .expect("matching message remains readable after replacement")
            .expect("matching message remains cached");
        assert!(
            cached_body.is_none(),
            "replacement survivors require a fresh body"
        );
        assert_eq!(
            requests
                .lock()
                .expect("synthetic request log mutex is healthy")
                .as_slice(),
            [MessageDeltaRequest::Continue(next_link)]
        );
        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn a_second_delta_reset_is_checkpointed_for_the_next_attempt() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let first_reset = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken=first"
                .to_owned(),
        );
        let second_reset = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken=second"
                .to_owned(),
        );
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(
            account.clone(),
            FixtureProvider::new(
                account.clone(),
                vec![cached_folder.clone()],
                [
                    Ok(MessageDeltaResponse::Reset(first_reset.clone())),
                    Ok(MessageDeltaResponse::Reset(second_reset.clone())),
                ],
                Err(MailOperationError::ServiceUnavailable),
            ),
            cache.clone(),
        );

        assert_eq!(
            workflow
                .synchronize_account(
                    UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                    &ignore_sync_progress,
                )
                .await,
            Err(MailWorkflowError::RepeatedDeltaReset)
        );
        let state = workflow
            .cached_sync_state(&folder_reference)
            .await
            .expect("latest reset checkpoint remains readable")
            .expect("folder state exists");
        assert_eq!(state.next_link(), Some(&second_reset));
        assert_eq!(state.full_sync_generation(), Some(2));
        assert!(state.is_rebuilding());
        drop(workflow);
        cache.close_for_tests().await;

        let cache = open_cache(&directory, account.clone()).await;
        let completed_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken=complete"
                .to_owned(),
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            [Ok(page_response(MessageDeltaPage::new(
                Vec::new(),
                MessageDeltaProgress::Complete(completed_link),
            )))],
            Err(MailOperationError::ServiceUnavailable),
        );
        let requests = Arc::clone(&provider.requests);
        let workflow = MailWorkflow::new(account.clone(), provider, cache.clone());
        workflow
            .synchronize_account(
                UNIX_EPOCH + Duration::from_secs(1_800_000_001),
                &ignore_sync_progress,
            )
            .await
            .expect("the next attempt resumes the latest reset checkpoint");
        assert_eq!(
            requests
                .lock()
                .expect("synthetic request log mutex is healthy")
                .as_slice(),
            [MessageDeltaRequest::Continue(second_reset)]
        );
        let state = workflow
            .cached_sync_state(&folder_reference)
            .await
            .expect("completed checkpoint remains readable")
            .expect("folder state exists");
        assert!(!state.is_rebuilding());
        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn stale_delta_responses_cannot_advance_a_replaced_checkpoint() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let cutoff = started_at - Duration::from_secs(90 * 24 * 60 * 60);
        let cache = open_cache(&directory, account.clone()).await;
        cache
            .replace_folders(&account, vec![cached_folder])
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
        let old_delta_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken=old"
                .to_owned(),
        );
        let baseline_page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Baseline message".to_owned(), started_at),
            }],
            MessageDeltaProgress::Complete(old_delta_link),
        );
        let baseline_state = cache
            .commit_delta_page(&folder_reference, &initial_state, baseline_page, started_at)
            .await
            .expect("baseline snapshot commits");
        let reset_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken="
                .to_owned(),
        );
        let replacement_state = cache
            .begin_full_resync(&folder_reference, &baseline_state, reset_link)
            .await
            .expect("replacement checkpoint starts");

        assert_eq!(
            cache
                .begin_full_resync(
                    &folder_reference,
                    &baseline_state,
                    MessageDeltaLink::from_opaque(
                        "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken=stale-reset"
                            .to_owned(),
                    ),
                )
                .await,
            Err(MailCacheError::SyncStateChanged)
        );
        assert_eq!(
            cache
                .commit_delta_page(
                    &folder_reference,
                    &baseline_state,
                    MessageDeltaPage::new(
                        vec![MessageDeltaChange::Upsert {
                            id: MessageId::try_from("stale-session-message".to_owned())
                                .expect("synthetic message ID is valid"),
                            patch: complete_patch("Stale page".to_owned(), started_at),
                        }],
                        MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken=stale-page"
                                .to_owned(),
                        )),
                    ),
                    started_at + Duration::from_secs(1),
                )
                .await,
            Err(MailCacheError::SyncStateChanged)
        );

        let stored_state = cache
            .load_sync_state(&folder_reference)
            .await
            .expect("replacement checkpoint remains readable")
            .expect("folder checkpoint exists");
        assert_eq!(stored_state, replacement_state);
        assert_eq!(stored_state.checkpoint_revision(), 2);
        assert!(stored_state.is_rebuilding());
        let messages = cache
            .list_messages(&folder_reference)
            .await
            .expect("the previous snapshot remains readable");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id(), &message_id());
        cache.close_for_tests().await;
    });
}

#[test]
fn response_budget_remains_cumulative_after_a_delta_reset() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let cached_folder = folder(&account);
        let folder_reference = cached_folder.reference().clone();
        let reset_link = MessageDeltaLink::from_opaque(
            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken="
                .to_owned(),
        );
        let pages = (0..8)
            .map(|page_index| {
                let next_link = MessageDeltaLink::from_opaque(format!(
                    "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$skiptoken=before-reset-{page_index}"
                ));
                Ok(MessageDeltaResponse::Page(MessageDeltaPageResponse::new(
                    MessageDeltaPage::new(Vec::new(), MessageDeltaProgress::Next(next_link)),
                    8 * 1024 * 1024,
                )))
            })
            .chain([
                Ok(MessageDeltaResponse::Reset(reset_link.clone())),
                Ok(MessageDeltaResponse::Page(MessageDeltaPageResponse::new(
                    MessageDeltaPage::new(
                        Vec::new(),
                        MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                            "https://graph.microsoft.com/v1.0/me/mailFolders/folder-inbox/messages/delta?$deltatoken=complete"
                                .to_owned(),
                        )),
                    ),
                    1,
                ))),
            ]);
        let provider = FixtureProvider::new(
            account.clone(),
            vec![cached_folder],
            pages,
            Err(MailOperationError::ServiceUnavailable),
        );
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(account.clone(), provider, cache.clone());

        assert_eq!(
            workflow
                .synchronize_account(
                    UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                    &ignore_sync_progress,
                )
                .await,
            Err(MailWorkflowError::ResponseLimitExceeded)
        );
        let state = workflow
            .cached_sync_state(&folder_reference)
            .await
            .expect("reset checkpoint remains readable")
            .expect("folder state exists");
        assert_eq!(state.next_link(), Some(&reset_link));
        assert!(state.is_rebuilding());
        drop(workflow);
        cache.close_for_tests().await;
    });
}
