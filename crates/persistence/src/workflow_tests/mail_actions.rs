use super::*;

#[test]
fn confirmed_message_move_reconciles_duplicate_cache_rows_and_preserves_body() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
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
                patch: complete_patch("Source subject".to_owned(), cutoff),
            }],
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-inbox".to_owned(),
            )),
        );
        let mut destination_patch = complete_patch("Destination subject".to_owned(), cutoff);
        destination_patch.set_subject(None);
        let destination_page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: destination_patch,
            }],
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-archive"
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
        let move_calls = Arc::clone(&provider.message_moves);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(account.clone(), provider, cache.clone());
        workflow
            .synchronize_account(started_at, &ignore_sync_progress)
            .await
            .expect("duplicate message rows are cached in separate folders");
        cache
            .store_message_body(
                &inbox_reference,
                &message_id(),
                MessageBody::new(MessageBodyType::Text, "Synthetic cached body".to_owned()),
            )
            .await
            .expect("source body is stored before the move");

        assert_eq!(
            workflow
                .move_message(&inbox_reference, &message_id(), &archive_reference)
                .await,
            Ok(einfach_mail_application::MessageMoveOutcome::ConfirmedAndCached)
        );
        assert_eq!(
            move_calls
                .lock()
                .expect("synthetic move log mutex is not poisoned")
                .len(),
            1
        );
        assert_eq!(
            cache
                .read_message(&inbox_reference, &message_id())
                .await
                .expect("source lookup succeeds"),
            None
        );
        let archived = cache
            .read_message(&archive_reference, &message_id())
            .await
            .expect("destination lookup succeeds")
            .expect("message remains in the destination folder");
        assert_eq!(archived.0.folder(), &archive_reference);
        assert_eq!(archived.0.metadata().subject(), Some("Source subject"));
        assert_eq!(
            archived
                .1
                .expect("source body is copied when destination body is missing")
                .content(),
            "Synthetic cached body"
        );
        drop(workflow);
        cache.close_for_tests().await;

        let reopened = open_cache(&directory, account).await;
        let archived_after_restart = reopened
            .read_message(&archive_reference, &message_id())
            .await
            .expect("archive row is readable after reopening")
            .expect("moved message persists in the archive");
        assert_eq!(
            archived_after_restart
                .1
                .expect("moved body persists after reopening")
                .content(),
            "Synthetic cached body"
        );
        reopened.close_for_tests().await;
    });
}

#[test]
fn message_move_rejects_a_destination_outside_the_visible_folder_tree() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account);
        let archive = archive_folder(&account);
        let inbox_reference = inbox.reference().clone();
        let hidden_reference = FolderReference::new(
            account.clone(),
            FolderId::try_from("hidden".to_owned()).expect("synthetic folder ID is valid"),
        );
        let started_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let cutoff = started_at - Duration::from_secs(90 * 24 * 60 * 60);
        let source_page = MessageDeltaPage::new(
            vec![MessageDeltaChange::Upsert {
                id: message_id(),
                patch: complete_patch("Synthetic subject".to_owned(), cutoff),
            }],
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-inbox".to_owned(),
            )),
        );
        let empty_archive_page = MessageDeltaPage::new(
            Vec::new(),
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-archive"
                    .to_owned(),
            )),
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![inbox, archive],
            [
                Ok(page_response(source_page)),
                Ok(page_response(empty_archive_page)),
            ],
            Err(MailOperationError::ServiceUnavailable),
        );
        let move_calls = Arc::clone(&provider.message_moves);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(account.clone(), provider, cache.clone());
        workflow
            .synchronize_account(started_at, &ignore_sync_progress)
            .await
            .expect("synthetic source message is cached");

        assert_eq!(
            workflow
                .move_message(&inbox_reference, &message_id(), &hidden_reference)
                .await,
            Err(MailWorkflowError::InvalidMoveDestination)
        );
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

#[test]
fn message_move_cache_failure_rolls_back_and_requests_sync() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
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
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-inbox".to_owned(),
            )),
        );
        let empty_destination_page = MessageDeltaPage::new(
            Vec::new(),
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-archive"
                    .to_owned(),
            )),
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![inbox, archive],
            [
                Ok(page_response(source_page)),
                Ok(page_response(empty_destination_page)),
            ],
            Err(MailOperationError::ServiceUnavailable),
        );
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(account.clone(), provider, cache.clone());
        workflow
            .synchronize_account(started_at, &ignore_sync_progress)
            .await
            .expect("synthetic source message is cached");
        cache.set_message_move_failure_for_tests(true).await;

        assert_eq!(
            workflow
                .move_message(&inbox_reference, &message_id(), &archive_reference)
                .await,
            Ok(einfach_mail_application::MessageMoveOutcome::ConfirmedNeedsSync)
        );
        assert!(
            cache
                .read_message(&inbox_reference, &message_id())
                .await
                .expect("source lookup succeeds after rollback")
                .is_some()
        );
        assert!(
            cache
                .read_message(&archive_reference, &message_id())
                .await
                .expect("destination lookup succeeds after rollback")
                .is_none()
        );
        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn unknown_message_move_does_not_change_the_cached_folder() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
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
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-inbox".to_owned(),
            )),
        );
        let empty_destination_page = MessageDeltaPage::new(
            Vec::new(),
            MessageDeltaProgress::Complete(MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$deltatoken=synthetic-archive"
                    .to_owned(),
            )),
        );
        let provider = FixtureProvider::new(
            account.clone(),
            vec![inbox, archive],
            [
                Ok(page_response(source_page)),
                Ok(page_response(empty_destination_page)),
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
        let workflow = MailWorkflow::new(account.clone(), provider, cache.clone());
        workflow
            .synchronize_account(started_at, &ignore_sync_progress)
            .await
            .expect("synthetic source message is cached");

        assert_eq!(
            workflow
                .move_message(&inbox_reference, &message_id(), &archive_reference)
                .await,
            Err(MailWorkflowError::Provider(
                MailOperationError::MoveOutcomeUnknown
            ))
        );
        assert!(
            cache
                .read_message(&inbox_reference, &message_id())
                .await
                .expect("source lookup succeeds")
                .is_some()
        );
        assert!(
            cache
                .read_message(&archive_reference, &message_id())
                .await
                .expect("destination lookup succeeds")
                .is_none()
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
