use einfach_mail_application::ReadStateUpdateOutcome;
use einfach_mail_domain::UnreadCount;

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
