use std::sync::Mutex;

use einfach_mail_application::ReadStateUpdateOutcome;
use einfach_mail_domain::{MailFolder, MailFolderKinds, UnreadCount};

use super::*;

type FixtureWorkflow = MailWorkflow<FixtureProvider, SqliteAccountCache>;

fn started_at() -> std::time::SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_800_000_000)
}

fn message(id: &str) -> MessageId {
    MessageId::try_from(id.to_owned()).expect("synthetic message ID is valid")
}

fn unread_message(id: &str) -> MessageDeltaChange {
    MessageDeltaChange::Upsert {
        id: message(id),
        patch: complete_patch("Synthetic subject".to_owned(), started_at()),
    }
}

/// Synchronizes an Inbox that holds three unread messages and reports `provider_unread`.
async fn workflow_with_three_unread_messages(
    cache: &SqliteAccountCache,
    provider_unread: u32,
) -> FixtureWorkflow {
    let account = account_id();
    let inbox = folder(&account).with_unread_count(Some(UnreadCount::new(provider_unread)));
    let page = completed_message_delta_page(
        "folder-metadata",
        vec![
            unread_message("message-1"),
            unread_message("message-2"),
            unread_message("message-3"),
        ],
    );
    let provider = FixtureProvider::new(
        account.clone(),
        vec![inbox],
        [Ok(page_response(page))],
        Err(MailOperationError::ServiceUnavailable),
    );
    let workflow = MailWorkflow::new(account, provider, cache.clone());
    workflow
        .synchronize_account(started_at(), &ignore_sync_progress)
        .await
        .expect("synthetic Inbox synchronizes");
    workflow
}

async fn inbox_unread_count(workflow: &FixtureWorkflow) -> Option<u32> {
    let inbox = folder(&account_id());
    workflow
        .list_cached_folders()
        .await
        .expect("cached folders are readable")
        .into_iter()
        .find(|cached| cached.reference() == inbox.reference())
        .expect("synthetic Inbox is cached")
        .unread_count()
        .map(UnreadCount::get)
}

#[test]
fn a_confirmed_read_state_changes_the_folder_unread_count_once() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account).reference().clone();
        let cache = open_cache(&directory, account).await;
        let workflow = workflow_with_three_unread_messages(&cache, 40).await;
        assert_eq!(inbox_unread_count(&workflow).await, Some(40));

        for (requested, expected_count) in [
            (ReadState::Read, 39),
            (ReadState::Read, 39),
            (ReadState::Unread, 40),
        ] {
            assert_eq!(
                workflow
                    .set_message_read_state(&inbox, &message("message-1"), requested)
                    .await,
                Ok(ReadStateUpdateOutcome::ConfirmedAndCached(requested))
            );
            assert_eq!(inbox_unread_count(&workflow).await, Some(expected_count));
        }

        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn a_confirmed_read_state_that_misses_the_cache_keeps_the_folder_unread_count() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account).reference().clone();
        let cache = open_cache(&directory, account).await;
        let workflow = workflow_with_three_unread_messages(&cache, 40).await;
        cache
            .execute_batch_for_tests(
                "CREATE TRIGGER fail_read_state_write
                 BEFORE UPDATE OF is_read ON mail_messages
                 BEGIN SELECT RAISE(ABORT, 'synthetic read-state failure'); END;",
            )
            .await
            .expect("synthetic failure trigger is installed");

        assert_eq!(
            workflow
                .set_message_read_state(&inbox, &message("message-1"), ReadState::Read)
                .await,
            Ok(ReadStateUpdateOutcome::ConfirmedNeedsSync {
                state: ReadState::Read,
                cache_error: MailCacheError::Unavailable,
            })
        );
        assert_eq!(inbox_unread_count(&workflow).await, Some(40));

        drop(workflow);
        cache.close_for_tests().await;
    });
}

/// Returns one empty final page for each folder that the synchronizations visit.
fn empty_final_pages(
    count: usize,
) -> impl Iterator<Item = Result<MessageDeltaResponse, MailOperationError>> {
    (0..count).map(|index| {
        Ok(page_response(completed_message_delta_page(
            &format!("empty-{index}"),
            Vec::new(),
        )))
    })
}

fn kinds(assignments: &[(MailFolderKind, &MailFolder)]) -> MailFolderKinds {
    let mut kinds = MailFolderKinds::new();
    for (kind, folder) in assignments {
        assert!(
            kinds.assign(*kind, folder.reference().folder_id().clone()),
            "synthetic assignments are one-to-one"
        );
    }
    kinds
}

fn set_folder_kinds_result(
    result: &Mutex<Result<MailFolderKinds, MailOperationError>>,
    next: Result<MailFolderKinds, MailOperationError>,
) {
    *result
        .lock()
        .expect("synthetic folder-kind result mutex is not poisoned") = next;
}

async fn cached_kind(workflow: &FixtureWorkflow, folder: &MailFolder) -> Option<MailFolderKind> {
    workflow
        .list_cached_folders()
        .await
        .expect("cached folders are readable")
        .into_iter()
        .find(|cached| cached.reference() == folder.reference())
        .expect("synthetic folder is cached")
        .kind()
}

#[test]
fn folder_kinds_resolve_once_and_again_only_when_a_new_folder_appears() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account);
        let archive = archive_folder(&account);
        let deleted_items = deleted_items_folder(&account);
        let added = MailFolder::new(
            FolderReference::new(
                account.clone(),
                FolderId::try_from("folder-added".to_owned())
                    .expect("synthetic folder ID is valid"),
            ),
            None,
            "Synthetic added folder".to_owned(),
        )
        .expect("synthetic added folder is valid");
        let first_listing = vec![inbox.clone(), archive.clone(), deleted_items.clone()];
        let later_listing = [first_listing.clone(), vec![added.clone()]].concat();
        let provider = FixtureProvider::new(
            account.clone(),
            later_listing,
            empty_final_pages(10),
            Err(MailOperationError::ServiceUnavailable),
        );
        provider
            .folder_snapshots
            .lock()
            .expect("synthetic folder snapshot mutex is not poisoned")
            .extend([first_listing.clone(), first_listing]);
        set_folder_kinds_result(
            &provider.folder_kinds_result,
            Ok(kinds(&[
                (MailFolderKind::Inbox, &inbox),
                (MailFolderKind::Archive, &archive),
            ])),
        );
        let resolutions = Arc::clone(&provider.folder_kinds_calls);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(account, provider, cache.clone());

        workflow
            .synchronize_account(started_at(), &ignore_sync_progress)
            .await
            .expect("first synchronization completes");
        assert_eq!(resolutions.load(Ordering::Relaxed), 1);
        assert_eq!(
            cached_kind(&workflow, &inbox).await,
            Some(MailFolderKind::Inbox)
        );
        assert_eq!(
            cached_kind(&workflow, &archive).await,
            Some(MailFolderKind::Archive)
        );
        assert_eq!(cached_kind(&workflow, &deleted_items).await, None);

        workflow
            .synchronize_account(started_at(), &ignore_sync_progress)
            .await
            .expect("second synchronization completes");
        assert_eq!(resolutions.load(Ordering::Relaxed), 1);

        workflow
            .synchronize_account(started_at(), &ignore_sync_progress)
            .await
            .expect("third synchronization completes");
        assert_eq!(resolutions.load(Ordering::Relaxed), 2);
        assert_eq!(
            cached_kind(&workflow, &inbox).await,
            Some(MailFolderKind::Inbox)
        );
        assert_eq!(cached_kind(&workflow, &added).await, None);

        drop(workflow);
        cache.close_for_tests().await;
    });
}

#[test]
fn a_failed_folder_kind_resolution_does_not_fail_the_sync_and_runs_again() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account);
        let provider = FixtureProvider::new(
            account.clone(),
            vec![inbox.clone()],
            empty_final_pages(2),
            Err(MailOperationError::ServiceUnavailable),
        );
        let kinds_result = Arc::clone(&provider.folder_kinds_result);
        set_folder_kinds_result(&kinds_result, Err(MailOperationError::ServiceUnavailable));
        let resolutions = Arc::clone(&provider.folder_kinds_calls);
        let cache = open_cache(&directory, account.clone()).await;
        let workflow = MailWorkflow::new(account, provider, cache.clone());

        let report = workflow
            .synchronize_account(started_at(), &ignore_sync_progress)
            .await
            .expect("synchronization completes without folder kinds");
        assert_eq!((report.folder_count(), report.page_count()), (1, 1));
        assert_eq!(resolutions.load(Ordering::Relaxed), 1);
        assert_eq!(cached_kind(&workflow, &inbox).await, None);

        set_folder_kinds_result(&kinds_result, Ok(kinds(&[(MailFolderKind::Inbox, &inbox)])));
        workflow
            .synchronize_account(started_at(), &ignore_sync_progress)
            .await
            .expect("next synchronization completes");
        assert_eq!(resolutions.load(Ordering::Relaxed), 2);
        assert_eq!(
            cached_kind(&workflow, &inbox).await,
            Some(MailFolderKind::Inbox)
        );

        drop(workflow);
        cache.close_for_tests().await;
    });
}
