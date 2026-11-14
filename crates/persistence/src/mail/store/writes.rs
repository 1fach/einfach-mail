use super::*;

pub(super) fn move_cached_message(
    transaction: &Transaction<'_>,
    source_folder: &FolderReference,
    destination_folder: &FolderReference,
    message_id: &MessageId,
) -> MailCacheResult<()> {
    if !folder_exists(transaction, destination_folder)? {
        return Err(CacheFailure::InvalidData.into());
    }
    insert_or_merge_destination_message(
        transaction,
        source_folder,
        destination_folder,
        message_id,
    )?;
    remove_source_message(transaction, source_folder, message_id)
}

fn insert_or_merge_destination_message(
    transaction: &Transaction<'_>,
    source_folder: &FolderReference,
    destination_folder: &FolderReference,
    message_id: &MessageId,
) -> MailCacheResult<()> {
    let inserted = transaction.execute(
        &MOVE_CACHED_MESSAGE_SQL,
        params![
            source_folder.account_id().as_str(),
            source_folder.folder_id().as_str(),
            message_id.as_str(),
            destination_folder.folder_id().as_str()
        ],
    )?;
    if inserted != 1 {
        return Err(MailCacheFailure::MessageNotCached);
    }
    Ok(())
}

fn remove_source_message(
    transaction: &Transaction<'_>,
    source_folder: &FolderReference,
    message_id: &MessageId,
) -> MailCacheResult<()> {
    let removed = transaction.execute(
        "DELETE FROM mail_messages
         WHERE account_id = ?1 AND folder_id = ?2 AND message_id = ?3",
        params![
            source_folder.account_id().as_str(),
            source_folder.folder_id().as_str(),
            message_id.as_str()
        ],
    )?;
    if removed != 1 {
        return Err(MailCacheFailure::MessageNotCached);
    }
    Ok(())
}

/// Copies a message into the destination folder.
///
/// When the destination already holds the message, the destination row keeps its values. It
/// takes the source value only for a nullable column that is NULL in the destination row.
static MOVE_CACHED_MESSAGE_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "INSERT INTO mail_messages (
            account_id, folder_id, message_id, {columns}, body_type, body_content
         )
         SELECT account_id, ?4, message_id, {columns}, body_type, body_content
         FROM mail_messages
         WHERE account_id = ?1 AND folder_id = ?2 AND message_id = ?3
         ON CONFLICT (account_id, folder_id, message_id) DO UPDATE SET
            {filled_columns},
            body_type = COALESCE(mail_messages.body_type, excluded.body_type),
            body_content = COALESCE(mail_messages.body_content, excluded.body_content)",
        columns = message_columns_sql(str::to_owned),
        filled_columns = nullable_message_columns_sql(|name| {
            format!("{name} = COALESCE(mail_messages.{name}, excluded.{name})")
        }),
    )
});
