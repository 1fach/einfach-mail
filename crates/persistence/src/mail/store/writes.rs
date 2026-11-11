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
        MOVE_CACHED_MESSAGE_SQL,
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

const MOVE_CACHED_MESSAGE_SQL: &str = "
    INSERT INTO mail_messages (
        account_id, folder_id, message_id, subject,
        from_display_name, from_address, sender_display_name, sender_address,
        received_seconds, received_nanoseconds, is_read, flag_status,
        body_type, body_content
    )
    SELECT account_id, ?4, message_id, subject,
           from_display_name, from_address, sender_display_name, sender_address,
           received_seconds, received_nanoseconds, is_read, flag_status,
           body_type, body_content
    FROM mail_messages
    WHERE account_id = ?1 AND folder_id = ?2 AND message_id = ?3
    ON CONFLICT (account_id, folder_id, message_id) DO UPDATE SET
        subject = COALESCE(mail_messages.subject, excluded.subject),
        from_display_name = COALESCE(mail_messages.from_display_name, excluded.from_display_name),
        from_address = COALESCE(mail_messages.from_address, excluded.from_address),
        sender_display_name = COALESCE(mail_messages.sender_display_name, excluded.sender_display_name),
        sender_address = COALESCE(mail_messages.sender_address, excluded.sender_address),
        flag_status = COALESCE(mail_messages.flag_status, excluded.flag_status),
        body_type = COALESCE(mail_messages.body_type, excluded.body_type),
        body_content = COALESCE(mail_messages.body_content, excluded.body_content)
";
