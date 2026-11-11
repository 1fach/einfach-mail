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
