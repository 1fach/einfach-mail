use super::super::messages::MESSAGE_COLUMNS;
use super::*;

const MESSAGE: &str = "message-1";

fn mailbox(address: &str, display_name: &str) -> Mailbox {
    Mailbox::try_new(address.to_owned(), Some(display_name.to_owned()))
        .expect("synthetic mailbox is valid")
}

fn preview(text: &str) -> Option<MessagePreview> {
    MessagePreview::from_untrusted(text)
}

fn fully_populated_patch() -> MessageMetadataPatch {
    let mut patch = MessageMetadataPatch::new();
    patch.set_subject(Some("Synthetic subject".to_owned()));
    patch.set_from(Some(mailbox("from@example.test", "Synthetic From")));
    patch.set_sender(Some(mailbox("sender@example.test", "Synthetic Sender")));
    patch.set_received_at(received_at());
    patch.set_read_state(ReadState::Read);
    patch.set_follow_up_flag_status(Some(FollowUpFlagStatus::Flagged));
    patch.set_preview(preview("Synthetic preview"));
    patch
}

fn patch_with_preview(text: &str) -> MessageMetadataPatch {
    let mut patch = complete_patch("Synthetic subject".to_owned(), received_at());
    patch.set_preview(preview(text));
    patch
}

/// Returns the metadata columns that are NULL in at least one cached message.
async fn null_metadata_columns(cache: &SqliteAccountCache) -> Vec<&'static str> {
    cache
        .client
        .conn_and_then(|connection| {
            let mut null_columns = Vec::new();
            for column in &MESSAGE_COLUMNS {
                let has_null: bool = connection.query_row(
                    &format!(
                        "SELECT EXISTS (SELECT 1 FROM mail_messages WHERE {} IS NULL)",
                        column.name()
                    ),
                    [],
                    |row| row.get(0),
                )?;
                if has_null {
                    null_columns.push(column.name());
                }
            }
            Ok::<_, CacheFailure>(null_columns)
        })
        .await
        .expect("synthetic message rows are readable")
}

async fn stored_preview(cache: &SqliteAccountCache, folder: &FolderReference) -> Option<String> {
    let (stored, _) = cache
        .read_message(folder, &message(MESSAGE))
        .await
        .expect("cached message is readable")
        .expect("message is cached");
    stored
        .metadata()
        .preview()
        .map(|preview| preview.as_str().to_owned())
}

#[test]
fn every_metadata_field_survives_store_list_read_and_move() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account).reference().clone();
        let archive = archive_folder(&account).reference().clone();
        let cache =
            cache_with_folders(&directory, vec![folder(&account), archive_folder(&account)]).await;
        let patch = fully_populated_patch();
        let stored = patch
            .clone()
            .into_complete_message(inbox.clone(), message(MESSAGE))
            .expect("synthetic patch sets every required field");
        let body = MessageBody::new(MessageBodyType::Text, "Synthetic body".to_owned());

        complete_round(&cache, &inbox, vec![upsert(MESSAGE, patch)]).await;
        cache
            .store_message_body(&inbox, &message(MESSAGE), body.clone())
            .await
            .expect("synthetic body is cached");

        assert_eq!(
            null_metadata_columns(&cache).await,
            Vec::<&str>::new(),
            "the synthetic message must set every metadata column"
        );
        assert_eq!(cache.list_messages(&inbox).await, Ok(vec![stored.clone()]));
        assert_eq!(
            cache.read_message(&inbox, &message(MESSAGE)).await,
            Ok(Some((stored.clone(), Some(body.clone()))))
        );

        cache
            .move_message(&inbox, &archive, &message(MESSAGE))
            .await
            .expect("synthetic message moves");

        let moved = MailMessage::new(archive.clone(), message(MESSAGE), stored.metadata().clone());
        assert_eq!(cache.list_messages(&inbox).await, Ok(Vec::new()));
        assert_eq!(cache.list_messages(&archive).await, Ok(vec![moved.clone()]));
        assert_eq!(
            cache.read_message(&archive, &message(MESSAGE)).await,
            Ok(Some((moved, Some(body))))
        );
        cache.close_for_tests().await;
    });
}

#[test]
fn a_patch_changes_the_stored_preview_only_when_it_sets_one() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account).reference().clone();
        let cache = cache_with_folders(&directory, vec![folder(&account)]).await;
        complete_round(
            &cache,
            &inbox,
            vec![upsert(MESSAGE, patch_with_preview("First preview"))],
        )
        .await;

        let mut read_state_only = MessageMetadataPatch::new();
        read_state_only.set_read_state(ReadState::Read);
        complete_round(&cache, &inbox, vec![upsert(MESSAGE, read_state_only)]).await;
        assert_eq!(
            stored_preview(&cache, &inbox).await.as_deref(),
            Some("First preview")
        );

        let mut replaced = MessageMetadataPatch::new();
        replaced.set_preview(preview("Second preview"));
        complete_round(&cache, &inbox, vec![upsert(MESSAGE, replaced)]).await;
        assert_eq!(
            stored_preview(&cache, &inbox).await.as_deref(),
            Some("Second preview")
        );

        let mut cleared = MessageMetadataPatch::new();
        cleared.set_preview(None);
        complete_round(&cache, &inbox, vec![upsert(MESSAGE, cleared)]).await;
        assert_eq!(stored_preview(&cache, &inbox).await, None);
        cache.close_for_tests().await;
    });
}

#[test]
fn a_move_keeps_the_destination_preview_and_fills_a_missing_one() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account).reference().clone();
        let archive = archive_folder(&account).reference().clone();
        let cache =
            cache_with_folders(&directory, vec![folder(&account), archive_folder(&account)]).await;
        complete_round(
            &cache,
            &archive,
            vec![upsert(MESSAGE, patch_with_preview("Destination preview"))],
        )
        .await;
        complete_round(
            &cache,
            &inbox,
            vec![upsert(MESSAGE, patch_with_preview("Source preview"))],
        )
        .await;

        cache
            .move_message(&inbox, &archive, &message(MESSAGE))
            .await
            .expect("synthetic message moves onto the destination copy");
        assert_eq!(
            stored_preview(&cache, &archive).await.as_deref(),
            Some("Destination preview")
        );

        let mut cleared = MessageMetadataPatch::new();
        cleared.set_preview(None);
        complete_round(&cache, &archive, vec![upsert(MESSAGE, cleared)]).await;
        complete_round(
            &cache,
            &inbox,
            vec![upsert(MESSAGE, patch_with_preview("Source preview"))],
        )
        .await;
        cache
            .move_message(&inbox, &archive, &message(MESSAGE))
            .await
            .expect("synthetic message moves onto the destination copy");
        assert_eq!(
            stored_preview(&cache, &archive).await.as_deref(),
            Some("Source preview")
        );
        cache.close_for_tests().await;
    });
}

#[test]
fn a_stored_preview_is_normalized_again_when_it_is_read() {
    smol::block_on(async {
        let directory = TempDir::new().expect("temporary directory is created");
        let account = account_id();
        let inbox = folder(&account).reference().clone();
        let cache = cache_with_folders(&directory, vec![folder(&account)]).await;
        complete_round(
            &cache,
            &inbox,
            vec![upsert(MESSAGE, patch_with_preview("Synthetic preview"))],
        )
        .await;

        cache
            .execute_batch_for_tests(
                "UPDATE mail_messages
                 SET preview = '  Synthetic' || char(10) || char(8238) || 'preview  ';",
            )
            .await
            .expect("synthetic unnormalized preview is written");
        assert_eq!(
            stored_preview(&cache, &inbox).await.as_deref(),
            Some("Synthetic preview")
        );
        assert_eq!(
            cache
                .list_messages(&inbox)
                .await
                .expect("cached messages are readable")[0]
                .metadata()
                .preview()
                .map(MessagePreview::as_str),
            Some("Synthetic preview")
        );

        cache
            .execute_batch_for_tests("UPDATE mail_messages SET preview = '   ';")
            .await
            .expect("synthetic blank preview is written");
        assert_eq!(stored_preview(&cache, &inbox).await, None);
        cache.close_for_tests().await;
    });
}
