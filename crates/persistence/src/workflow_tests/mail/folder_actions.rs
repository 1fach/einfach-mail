#[test]
fn delete_message_requires_a_current_sync_before_resolving_deleted_items() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let provider = FixtureProvider::new(
            account.clone(),
            Vec::new(),
            [],
            Err(MailOperationError::ServiceUnavailable),
        );
        let folder_calls = Arc::clone(&provider.well_known_folder_calls);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(account.clone(), provider, cache.clone());
        let source = folder(&account).reference().clone();

        assert_eq!(
            workflow
                .move_message_to_well_known_folder(
                    &source,
                    &message_id(),
                    WellKnownMailFolder::DeletedItems
                )
                .await,
            Err(MailWorkflowError::AccountSyncRequired)
        );
        assert_eq!(folder_calls.load(Ordering::Relaxed), 0);
        cache.close_for_tests().await;
    });
}

#[test]
fn delete_message_uses_visible_deleted_items_id_and_preserves_cached_body() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account);
        let deleted = deleted_items_folder(&account);
        let inbox_reference = inbox.reference().clone();
        let deleted_reference = deleted.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let source_page = completed_message_delta_page(
            "delete-source",
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
                    "delete-destination",
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
            .expect("synthetic inbox and Deleted Items sync");
        cache
            .store_message_body(
                &inbox_reference,
                &message_id(),
                MessageBody::new(MessageBodyType::Text, "Synthetic cached body".to_owned()),
            )
            .await
            .expect("synthetic message body is cached before deletion");

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
        assert_eq!(folder_calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            move_calls
                .lock()
                .expect("synthetic move log mutex is not poisoned")
                .as_slice(),
            &[(
                inbox_reference.clone(),
                message_id(),
                deleted_reference.clone()
            )]
        );
        assert_eq!(
            cache
                .read_message(&inbox_reference, &message_id())
                .await
                .expect("source lookup succeeds after deletion"),
            None
        );
        let deleted_message = cache
            .read_message(&deleted_reference, &message_id())
            .await
            .expect("Deleted Items lookup succeeds")
            .expect("message is cached in Deleted Items");
        assert_eq!(
            deleted_message
                .1
                .expect("encrypted body moves with its message")
                .content(),
            "Synthetic cached body"
        );
        drop(workflow);
        cache.close_for_tests().await;

        let reopened = open_cache(&directory, account).await;
        let persisted = reopened
            .read_message(&deleted_reference, &message_id())
            .await
            .expect("Deleted Items message remains readable after reopening")
            .expect("deleted message persists in the encrypted cache");
        assert_eq!(
            persisted
                .1
                .expect("encrypted body persists after reopening")
                .content(),
            "Synthetic cached body"
        );
        reopened.close_for_tests().await;
    });
}

#[test]
fn archive_action_resolves_visible_archive_and_preserves_cached_body() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account);
        let archive = archive_folder(&account);
        let inbox_reference = inbox.reference().clone();
        let archive_reference = archive.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let source_page = completed_message_delta_page(
            "archive-action-source",
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Synthetic subject".to_owned(), started_at),
            }],
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![inbox, archive],
            [
                Ok(page_response(source_page)),
                Ok(page_response(completed_message_delta_page(
                    "archive-action-destination",
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
            .expect("synthetic inbox and Archive sync");
        cache
            .store_message_body(
                &inbox_reference,
                &message_id(),
                MessageBody::new(MessageBodyType::Text, "Synthetic archived body".to_owned()),
            )
            .await
            .expect("synthetic message body is cached before archiving");

        let outcome = workflow
            .move_message_to_well_known_folder(
                &inbox_reference,
                &message_id(),
                WellKnownMailFolder::Archive,
            )
            .await
            .expect("Archive action resolves and confirms the visible well-known folder");

        assert_eq!(outcome.destination_folder(), &archive_reference);
        assert_eq!(
            outcome.cache_outcome(),
            einfach_mail_application::MessageMoveOutcome::ConfirmedAndCached
        );
        assert_eq!(folder_calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            move_calls
                .lock()
                .expect("synthetic move log mutex is not poisoned")
                .as_slice(),
            &[(
                inbox_reference.clone(),
                message_id(),
                archive_reference.clone(),
            )]
        );
        assert!(
            cache
                .read_message(&inbox_reference, &message_id())
                .await
                .expect("source cache lookup succeeds after archiving")
                .is_none()
        );
        let archived_message = cache
            .read_message(&archive_reference, &message_id())
            .await
            .expect("Archive cache lookup succeeds")
            .expect("message is cached in Archive");
        assert_eq!(
            archived_message
                .1
                .expect("encrypted body moves with the message")
                .content(),
            "Synthetic archived body"
        );
        cache.close_for_tests().await;
    });
}

#[test]
fn archive_action_rejects_a_message_already_in_archive() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let archive = archive_folder(&account);
        let archive_reference = archive.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let page = completed_message_delta_page(
            "already-in-archive",
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Synthetic subject".to_owned(), started_at),
            }],
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![archive],
            [Ok(page_response(page))],
            Err(MailOperationError::ServiceUnavailable),
        );
        let folder_calls = Arc::clone(&provider.well_known_folder_calls);
        let move_calls = Arc::clone(&provider.message_moves);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(account.clone(), provider, cache.clone());
        workflow
            .synchronize_account(started_at, &ignore_sync_progress)
            .await
            .expect("synthetic Archive sync completes");

        assert_eq!(
            workflow
                .move_message_to_well_known_folder(
                    &archive_reference,
                    &message_id(),
                    WellKnownMailFolder::Archive,
                )
                .await,
            Err(MailWorkflowError::AlreadyInWellKnownFolder(
                WellKnownMailFolder::Archive
            ))
        );
        assert_eq!(folder_calls.load(Ordering::Relaxed), 1);
        assert!(
            move_calls
                .lock()
                .expect("synthetic move log mutex is not poisoned")
                .is_empty()
        );
        cache.close_for_tests().await;
    });
}

#[test]
fn well_known_folder_rejects_a_resolved_folder_outside_the_visible_tree() {
    smol::block_on(async {
        for destination in [
            WellKnownMailFolder::Archive,
            WellKnownMailFolder::DeletedItems,
        ] {
            let directory = TempDir::new().expect("temporary directory is created");
            let account = account_id();
            let inbox = folder(&account);
            let inbox_reference = inbox.reference().clone();
            let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
            let source_page = completed_message_delta_page(
                "invisible-special-folder-source",
                vec![MessageDeltaChange::Upsert {
                    id: message_id(),
                    patch: complete_patch("Synthetic subject".to_owned(), started_at),
                }],
            );
            let mut provider = FixtureProvider::new(
                account.clone(),
                vec![
                    inbox,
                    archive_folder(&account),
                    deleted_items_folder(&account),
                ],
                [
                    Ok(page_response(source_page)),
                    Ok(page_response(completed_message_delta_page(
                        "invisible-special-folder-archive",
                        Vec::new(),
                    ))),
                    Ok(page_response(completed_message_delta_page(
                        "invisible-special-folder-deleted",
                        Vec::new(),
                    ))),
                ],
                Err(MailOperationError::ServiceUnavailable),
            );
            let not_visible =
                FolderId::try_from("not-visible".to_owned()).expect("synthetic folder ID is valid");
            match destination {
                WellKnownMailFolder::Archive => provider.archive_folder_id = not_visible,
                WellKnownMailFolder::DeletedItems => provider.deleted_items_folder_id = not_visible,
            }
            let folder_calls = Arc::clone(&provider.well_known_folder_calls);
            let move_calls = Arc::clone(&provider.message_moves);
            let cache = open_cache(&directory, account.clone()).await;
            let workflow = MailWorkflow::new(account.clone(), provider, cache.clone());
            workflow
                .synchronize_account(started_at, &ignore_sync_progress)
                .await
                .expect("synthetic account sync completes");

            assert_eq!(
                workflow
                    .move_message_to_well_known_folder(
                        &inbox_reference,
                        &message_id(),
                        destination,
                    )
                    .await,
                Err(MailWorkflowError::WellKnownFolderUnavailable(destination))
            );
            assert_eq!(folder_calls.load(Ordering::Relaxed), 1);
            assert!(
                move_calls
                    .lock()
                    .expect("synthetic move log mutex is not poisoned")
                    .is_empty()
            );
            assert!(
                cache
                    .read_message(&inbox_reference, &message_id())
                    .await
                    .expect("source cache lookup succeeds")
                    .is_some()
            );
            cache.close_for_tests().await;
        }
    });
}

#[test]
fn delete_message_rejects_a_message_already_in_deleted_items() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let deleted = deleted_items_folder(&account);
        let deleted_reference = deleted.reference().clone();
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let page = completed_message_delta_page(
            "already-deleted",
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Synthetic subject".to_owned(), started_at),
            }],
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![deleted],
            [Ok(page_response(page))],
            Err(MailOperationError::ServiceUnavailable),
        );
        let folder_calls = Arc::clone(&provider.well_known_folder_calls);
        let move_calls = Arc::clone(&provider.message_moves);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(account.clone(), provider, cache.clone());
        workflow
            .synchronize_account(started_at, &ignore_sync_progress)
            .await
            .expect("synthetic Deleted Items sync completes");

        assert_eq!(
            workflow
                .move_message_to_well_known_folder(
                    &deleted_reference,
                    &message_id(),
                    WellKnownMailFolder::DeletedItems,
                )
                .await,
            Err(MailWorkflowError::AlreadyInWellKnownFolder(
                WellKnownMailFolder::DeletedItems
            ))
        );
        assert_eq!(folder_calls.load(Ordering::Relaxed), 1);
        assert!(
            move_calls
                .lock()
                .expect("synthetic move log mutex is not poisoned")
                .is_empty()
        );
        assert!(
            cache
                .read_message(&deleted_reference, &message_id())
                .await
                .expect("Deleted Items cache lookup succeeds")
                .is_some()
        );
        cache.close_for_tests().await;
    });
}
