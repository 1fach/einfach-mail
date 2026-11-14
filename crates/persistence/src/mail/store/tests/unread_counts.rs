use super::*;

fn reporting(folder: MailFolder, unread: u32) -> MailFolder {
    folder.with_unread_count(Some(UnreadCount::new(unread)))
}

fn unread_message(id: &str) -> MessageDeltaChange {
    upsert(
        id,
        complete_patch("Synthetic subject".to_owned(), received_at()),
    )
}

fn read_message(id: &str) -> MessageDeltaChange {
    let mut patch = complete_patch("Synthetic subject".to_owned(), received_at());
    patch.set_read_state(ReadState::Read);
    upsert(id, patch)
}

fn three_unread_messages() -> Vec<MessageDeltaChange> {
    vec![
        unread_message("message-1"),
        unread_message("message-2"),
        unread_message("message-3"),
    ]
}

async fn unread_count(cache: &SqliteAccountCache, folder: &FolderReference) -> Option<u32> {
    cache
        .list_folders(folder.account_id())
        .await
        .expect("cached folders are readable")
        .into_iter()
        .find(|cached| cached.reference() == folder)
        .expect("synthetic folder is cached")
        .unread_count()
        .map(UnreadCount::get)
}

#[test]
fn a_folder_shows_the_provider_count_once_its_round_completes() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account).reference().clone();
        let cache = cache_with_folders(&directory, vec![reporting(folder(&account), 40)]).await;
        assert_eq!(unread_count(&cache, &inbox).await, None);

        commit_unfinished_page(&cache, &inbox, three_unread_messages()).await;
        assert_eq!(unread_count(&cache, &inbox).await, None);

        complete_round(&cache, &inbox, Vec::new()).await;
        assert_eq!(unread_count(&cache, &inbox).await, Some(40));
        cache.close_for_tests().await;
    });
}

#[test]
fn a_folder_without_a_provider_count_shows_no_count() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account).reference().clone();
        let cache = cache_with_folders(&directory, vec![folder(&account)]).await;

        complete_round(&cache, &inbox, three_unread_messages()).await;

        assert_eq!(unread_count(&cache, &inbox).await, None);
        cache.close_for_tests().await;
    });
}

#[test]
fn the_count_never_drops_below_the_cached_unread_messages() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account).reference().clone();
        let cache = cache_with_folders(&directory, vec![reporting(folder(&account), 1)]).await;

        complete_round(&cache, &inbox, three_unread_messages()).await;

        assert_eq!(unread_count(&cache, &inbox).await, Some(3));
        cache.close_for_tests().await;
    });
}

#[test]
fn a_move_carries_an_unread_message_to_the_destination_count() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account).reference().clone();
        let archive = archive_folder(&account).reference().clone();
        let cache = cache_with_folders(
            &directory,
            vec![
                reporting(folder(&account), 40),
                reporting(archive_folder(&account), 5),
            ],
        )
        .await;
        complete_round(&cache, &inbox, three_unread_messages()).await;
        complete_round(&cache, &archive, Vec::new()).await;

        cache
            .move_message(&inbox, &archive, &message("message-1"))
            .await
            .expect("synthetic unread message moves");

        assert_eq!(unread_count(&cache, &inbox).await, Some(39));
        assert_eq!(unread_count(&cache, &archive).await, Some(6));
        cache.close_for_tests().await;
    });
}

#[test]
fn a_move_onto_an_existing_copy_keeps_the_destination_read_state_in_the_count() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account).reference().clone();
        let archive = archive_folder(&account).reference().clone();
        let cache = cache_with_folders(
            &directory,
            vec![
                reporting(folder(&account), 40),
                reporting(archive_folder(&account), 5),
            ],
        )
        .await;
        complete_round(&cache, &inbox, three_unread_messages()).await;
        complete_round(&cache, &archive, vec![read_message("message-1")]).await;

        cache
            .move_message(&inbox, &archive, &message("message-1"))
            .await
            .expect("synthetic message moves onto its destination copy");

        assert_eq!(unread_count(&cache, &inbox).await, Some(39));
        assert_eq!(unread_count(&cache, &archive).await, Some(5));
        cache.close_for_tests().await;
    });
}

#[test]
fn delta_changes_move_the_count_before_the_round_completes() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account).reference().clone();
        let cache = cache_with_folders(&directory, vec![reporting(folder(&account), 40)]).await;
        complete_round(&cache, &inbox, three_unread_messages()).await;

        commit_unfinished_page(
            &cache,
            &inbox,
            vec![MessageDeltaChange::Removed {
                id: message("message-1"),
            }],
        )
        .await;
        assert_eq!(unread_count(&cache, &inbox).await, Some(39));

        commit_unfinished_page(&cache, &inbox, vec![read_message("message-2")]).await;
        assert_eq!(unread_count(&cache, &inbox).await, Some(38));

        commit_unfinished_page(&cache, &inbox, vec![unread_message("message-4")]).await;
        assert_eq!(unread_count(&cache, &inbox).await, Some(39));
        cache.close_for_tests().await;
    });
}

#[test]
fn only_an_accepted_final_page_takes_a_new_provider_count() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account).reference().clone();
        let cache = cache_with_folders(&directory, vec![reporting(folder(&account), 40)]).await;
        let first_state = sync_state(&cache, &inbox).await;
        complete_round(&cache, &inbox, three_unread_messages()).await;
        assert_eq!(unread_count(&cache, &inbox).await, Some(40));

        cache
            .replace_folders(&account, vec![reporting(folder(&account), 50)])
            .await
            .expect("a later folder listing is cached");
        assert_eq!(unread_count(&cache, &inbox).await, Some(40));

        let stale_page = completed_message_delta_page("stale", Vec::new());
        assert_eq!(
            MailCache::commit_delta_page(&cache, &inbox, &first_state, stale_page, received_at())
                .await,
            Err(MailCacheError::SyncStateChanged)
        );
        assert_eq!(unread_count(&cache, &inbox).await, Some(40));

        commit_unfinished_page(&cache, &inbox, Vec::new()).await;
        assert_eq!(unread_count(&cache, &inbox).await, Some(40));

        complete_round(&cache, &inbox, Vec::new()).await;
        assert_eq!(unread_count(&cache, &inbox).await, Some(50));
        cache.close_for_tests().await;
    });
}

#[test]
fn a_replacement_scan_takes_the_provider_count_after_it_deletes_unseen_messages() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account).reference().clone();
        let cache = cache_with_folders(&directory, vec![reporting(folder(&account), 40)]).await;
        complete_round(&cache, &inbox, three_unread_messages()).await;
        let state = sync_state(&cache, &inbox).await;
        cache
            .begin_full_resync(
                &inbox,
                &state,
                MessageDeltaLink::from_opaque(
                    "https://graph.microsoft.com/v1.0/messages?$deltatoken=".to_owned(),
                ),
            )
            .await
            .expect("synthetic replacement scan starts");

        complete_round(&cache, &inbox, vec![unread_message("message-1")]).await;

        assert_eq!(
            cache
                .list_messages(&inbox)
                .await
                .expect("cached messages are readable")
                .len(),
            1
        );
        assert_eq!(unread_count(&cache, &inbox).await, Some(40));
        cache.close_for_tests().await;
    });
}
