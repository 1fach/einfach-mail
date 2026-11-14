use super::*;

pub(super) fn read_sync_state(
    connection: &Connection,
    folder: &FolderReference,
) -> CacheResult<Option<MailSyncState>> {
    let raw = connection
        .query_row(
            "SELECT a.query_version, a.cutoff_seconds, a.cutoff_nanoseconds,
                    s.delta_link, s.next_link, s.last_sync_seconds, s.last_sync_nanoseconds,
                    s.full_sync_generation, s.full_sync_active, s.checkpoint_revision
             FROM mail_sync_state s
             JOIN mail_account_sync_scope a USING (account_id)
             WHERE s.account_id = ?1 AND s.folder_id = ?2",
            params![folder.account_id().as_str(), folder.folder_id().as_str()],
            |row| {
                Ok(RawSyncState {
                    query_version: row.get(0)?,
                    cutoff_seconds: row.get(1)?,
                    cutoff_nanoseconds: row.get(2)?,
                    delta_link: row.get(3)?,
                    next_link: row.get(4)?,
                    sync_seconds: row.get(5)?,
                    sync_nanoseconds: row.get(6)?,
                    full_sync_generation: row.get(7)?,
                    full_sync_active: row.get(8)?,
                    checkpoint_revision: row.get(9)?,
                })
            },
        )
        .optional()?;
    raw.map(decode_sync_state).transpose()
}

struct RawSyncState {
    query_version: i64,
    cutoff_seconds: i64,
    cutoff_nanoseconds: i64,
    delta_link: Option<String>,
    next_link: Option<String>,
    sync_seconds: Option<i64>,
    sync_nanoseconds: Option<i64>,
    full_sync_generation: i64,
    full_sync_active: i64,
    checkpoint_revision: i64,
}

fn decode_sync_state(raw: RawSyncState) -> CacheResult<MailSyncState> {
    if raw.query_version != schema::MESSAGE_QUERY_VERSION {
        return Err(CacheFailure::InvalidData);
    }
    let cutoff = system_time_from_parts(raw.cutoff_seconds, raw.cutoff_nanoseconds)?;
    let last_synchronized_at = match (raw.sync_seconds, raw.sync_nanoseconds) {
        (Some(seconds), Some(nanoseconds)) => Some(system_time_from_parts(seconds, nanoseconds)?),
        (None, None) => None,
        _ => return Err(CacheFailure::InvalidData),
    };
    if raw.full_sync_generation < 0 {
        return Err(CacheFailure::InvalidData);
    }
    if raw.checkpoint_revision < 0 {
        return Err(CacheFailure::InvalidData);
    }
    let full_sync_generation = match raw.full_sync_active {
        0 => None,
        1 if raw.full_sync_generation > 0 => {
            Some(u64::try_from(raw.full_sync_generation).map_err(|_| CacheFailure::InvalidData)?)
        }
        _ => return Err(CacheFailure::InvalidData),
    };
    MailSyncState::from_checkpoint(
        cutoff,
        raw.delta_link.map(MessageDeltaLink::from_opaque),
        raw.next_link.map(MessageDeltaLink::from_opaque),
        last_synchronized_at,
        full_sync_generation,
        u64::try_from(raw.checkpoint_revision).map_err(|_| CacheFailure::InvalidData)?,
    )
    .map_err(|_| CacheFailure::InvalidData)
}

pub(super) fn ensure_account_scope_row(
    transaction: &Transaction<'_>,
    folder: &FolderReference,
    cutoff_seconds: i64,
    cutoff_nanoseconds: i64,
) -> CacheResult<()> {
    transaction.execute(
        "INSERT OR IGNORE INTO mail_account_sync_scope (
            account_id, query_version, cutoff_seconds, cutoff_nanoseconds
         ) VALUES (?1, ?2, ?3, ?4)",
        params![
            folder.account_id().as_str(),
            schema::MESSAGE_QUERY_VERSION,
            cutoff_seconds,
            cutoff_nanoseconds
        ],
    )?;
    let query_version: i64 = transaction.query_row(
        "SELECT query_version FROM mail_account_sync_scope WHERE account_id = ?1",
        [folder.account_id().as_str()],
        |row| row.get(0),
    )?;
    if query_version != schema::MESSAGE_QUERY_VERSION {
        return Err(CacheFailure::InvalidData);
    }
    Ok(())
}

pub(super) fn apply_delta_change(
    connection: &Connection,
    folder: &FolderReference,
    cutoff: SystemTime,
    full_sync_generation: Option<i64>,
    change: MessageDeltaChange,
) -> CacheResult<()> {
    match change {
        MessageDeltaChange::Removed { id } => {
            remove_or_defer_message(connection, folder, &id, full_sync_generation)
        }
        MessageDeltaChange::Upsert { id, patch } => {
            apply_message_patch(connection, folder, cutoff, full_sync_generation, id, patch)
        }
    }
}

fn remove_or_defer_message(
    connection: &Connection,
    folder: &FolderReference,
    message_id: &MessageId,
    full_sync_generation: Option<i64>,
) -> CacheResult<()> {
    if full_sync_generation.is_some() {
        connection.execute(
            "UPDATE mail_messages SET full_sync_generation = NULL
             WHERE account_id = ?1 AND folder_id = ?2 AND message_id = ?3",
            params![
                folder.account_id().as_str(),
                folder.folder_id().as_str(),
                message_id.as_str()
            ],
        )?;
        Ok(())
    } else {
        remove_message(connection, folder, message_id)
    }
}

fn apply_message_patch(
    connection: &Connection,
    folder: &FolderReference,
    cutoff: SystemTime,
    full_sync_generation: Option<i64>,
    id: MessageId,
    patch: einfach_mail_domain::MessageMetadataPatch,
) -> CacheResult<()> {
    let mut cached = read_cached_metadata(connection, folder, &id)?;
    let Some(message) = cached.as_mut() else {
        if let Some(message) = patch.into_complete_message(folder.clone(), id)
            && message.metadata().received_at() >= &cutoff
        {
            return store_message_metadata(connection, &message, full_sync_generation);
        }
        return Ok(());
    };
    message.apply_patch(patch);
    if message.metadata().received_at() < &cutoff {
        return remove_or_defer_message(connection, folder, message.id(), full_sync_generation);
    }
    store_message_metadata(connection, message, full_sync_generation)
}

fn read_cached_metadata(
    connection: &Connection,
    folder: &FolderReference,
    message_id: &MessageId,
) -> CacheResult<Option<MailMessage>> {
    let raw = connection
        .query_row(
            &READ_MESSAGE_METADATA_SQL,
            params![
                folder.account_id().as_str(),
                folder.folder_id().as_str(),
                message_id.as_str()
            ],
            raw_message_from_row,
        )
        .optional()?;
    raw.map(|raw| decode_message(folder, raw)).transpose()
}

/// Inserts a message or replaces its metadata, and clears its cached body.
static STORE_MESSAGE_METADATA_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "INSERT INTO mail_messages (
            account_id, folder_id, message_id, {columns}, full_sync_generation
         ) VALUES (:account_id, :folder_id, :message_id, {values}, :full_sync_generation)
         ON CONFLICT (account_id, folder_id, message_id) DO UPDATE SET
            {replacements},
            full_sync_generation = excluded.full_sync_generation,
            body_type = NULL,
            body_content = NULL",
        columns = message_columns_sql(str::to_owned),
        values = message_columns_sql(|name| format!(":{name}")),
        replacements = message_columns_sql(|name| format!("{name} = excluded.{name}")),
    )
});

fn store_message_metadata(
    connection: &Connection,
    message: &MailMessage,
    full_sync_generation: Option<i64>,
) -> CacheResult<()> {
    let metadata = message.metadata();
    let (from_name, from_address) = mailbox_fields(metadata.origin().from_mailbox());
    let (sender_name, sender_address) = mailbox_fields(metadata.origin().sender());
    let (received_seconds, received_nanoseconds) = system_time_parts(*metadata.received_at())?;
    connection.execute(
        &STORE_MESSAGE_METADATA_SQL,
        named_params! {
            ":account_id": message.folder().account_id().as_str(),
            ":folder_id": message.folder().folder_id().as_str(),
            ":message_id": message.id().as_str(),
            ":subject": metadata.subject(),
            ":from_display_name": from_name,
            ":from_address": from_address,
            ":sender_display_name": sender_name,
            ":sender_address": sender_address,
            ":received_seconds": received_seconds,
            ":received_nanoseconds": received_nanoseconds,
            ":is_read": matches!(metadata.read_state(), ReadState::Read),
            ":flag_status": metadata
                .follow_up_flag_status()
                .map(follow_up_flag_status_name),
            ":preview": metadata.preview().map(MessagePreview::as_str),
            ":full_sync_generation": full_sync_generation,
        },
    )?;
    Ok(())
}

fn mailbox_fields(mailbox: Option<&Mailbox>) -> (Option<&str>, Option<&str>) {
    mailbox.map_or((None, None), |mailbox| {
        (mailbox.display_name(), Some(mailbox.address()))
    })
}

fn remove_message(
    connection: &Connection,
    folder: &FolderReference,
    message_id: &MessageId,
) -> CacheResult<()> {
    connection.execute(
        "DELETE FROM mail_messages
         WHERE account_id = ?1 AND folder_id = ?2 AND message_id = ?3",
        params![
            folder.account_id().as_str(),
            folder.folder_id().as_str(),
            message_id.as_str()
        ],
    )?;
    Ok(())
}

pub(super) fn begin_full_resync_transaction(
    connection: &mut Connection,
    folder: &FolderReference,
    expected_state: &MailSyncState,
    reset_link: &str,
) -> MailCacheResult<MailSyncState> {
    let transaction = begin_checkpoint_write(connection)?;
    let state = read_sync_state(&transaction, folder)?.ok_or(CacheFailure::InvalidData)?;
    if &state != expected_state {
        return Err(MailCacheFailure::SyncStateChanged);
    }
    let generation = state
        .full_sync_generation()
        .unwrap_or_default()
        .checked_add(1)
        .and_then(|generation| i64::try_from(generation).ok())
        .ok_or(CacheFailure::InvalidData)?;
    let checkpoint_revision = state
        .checkpoint_revision()
        .checked_add(1)
        .and_then(|revision| i64::try_from(revision).ok())
        .ok_or(CacheFailure::InvalidData)?;
    let updated_rows = transaction.execute(
        "UPDATE mail_sync_state
         SET full_sync_generation = ?3, full_sync_active = 1, next_link = ?4,
             checkpoint_revision = ?5
         WHERE account_id = ?1 AND folder_id = ?2",
        params![
            folder.account_id().as_str(),
            folder.folder_id().as_str(),
            generation,
            reset_link,
            checkpoint_revision
        ],
    )?;
    if updated_rows != 1 {
        return Err(CacheFailure::InvalidData.into());
    }
    transaction.commit()?;
    read_sync_state(connection, folder)?.ok_or(CacheFailure::InvalidData.into())
}

/// Starts a write transaction that serializes checkpoint comparisons.
pub(super) fn begin_checkpoint_write(connection: &mut Connection) -> CacheResult<Transaction<'_>> {
    Ok(connection.transaction_with_behavior(TransactionBehavior::Immediate)?)
}

fn active_full_sync_generation(
    transaction: &Transaction<'_>,
    folder: &FolderReference,
) -> CacheResult<Option<i64>> {
    let (active, generation) = transaction.query_row(
        "SELECT full_sync_active, full_sync_generation FROM mail_sync_state
         WHERE account_id = ?1 AND folder_id = ?2",
        params![folder.account_id().as_str(), folder.folder_id().as_str()],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    )?;
    match (active, generation) {
        (0, generation) if generation >= 0 => Ok(None),
        (1, generation) if generation > 0 => Ok(Some(generation)),
        _ => Err(CacheFailure::InvalidData),
    }
}

fn reconcile_full_sync(
    transaction: &Transaction<'_>,
    folder: &FolderReference,
    generation: i64,
) -> CacheResult<()> {
    transaction.execute(
        "DELETE FROM mail_messages
         WHERE account_id = ?1 AND folder_id = ?2 AND full_sync_generation IS NOT ?3",
        params![
            folder.account_id().as_str(),
            folder.folder_id().as_str(),
            generation
        ],
    )?;
    transaction.execute(
        "UPDATE mail_messages SET full_sync_generation = NULL
         WHERE account_id = ?1 AND folder_id = ?2 AND full_sync_generation = ?3",
        params![
            folder.account_id().as_str(),
            folder.folder_id().as_str(),
            generation
        ],
    )?;
    transaction.execute(
        "UPDATE mail_messages SET body_type = NULL, body_content = NULL
         WHERE account_id = ?1 AND folder_id = ?2",
        params![folder.account_id().as_str(), folder.folder_id().as_str()],
    )?;
    Ok(())
}

pub(super) fn update_sync_progress(
    transaction: &Transaction<'_>,
    folder: &FolderReference,
    progress: MessageDeltaProgress,
    completed_at: (i64, i64),
    checkpoint_revision: i64,
) -> CacheResult<()> {
    match progress {
        MessageDeltaProgress::Next(link) => {
            transaction.execute(
                "UPDATE mail_sync_state SET next_link = ?3, checkpoint_revision = ?4
                 WHERE account_id = ?1 AND folder_id = ?2",
                params![
                    folder.account_id().as_str(),
                    folder.folder_id().as_str(),
                    link.as_str(),
                    checkpoint_revision
                ],
            )?;
        }
        MessageDeltaProgress::Complete(link) => {
            if let Some(generation) = active_full_sync_generation(transaction, folder)? {
                reconcile_full_sync(transaction, folder, generation)?;
            }
            // The reconcile deletes cached messages, so the re-baseline counts unread
            // messages only after it.
            rebaseline_uncached_unread(transaction, folder)?;
            transaction.execute(
                "UPDATE mail_sync_state
                 SET delta_link = ?3, next_link = NULL,
                     last_sync_seconds = ?4, last_sync_nanoseconds = ?5,
                     full_sync_active = 0, checkpoint_revision = ?6
                 WHERE account_id = ?1 AND folder_id = ?2",
                params![
                    folder.account_id().as_str(),
                    folder.folder_id().as_str(),
                    link.as_str(),
                    completed_at.0,
                    completed_at.1,
                    checkpoint_revision
                ],
            )?;
            transaction.execute(
                "DELETE FROM mail_folder_write_sync_requirement
                 WHERE account_id = ?1 AND folder_id = ?2",
                params![folder.account_id().as_str(), folder.folder_id().as_str()],
            )?;
        }
    }
    Ok(())
}

pub(super) fn body_type_name(body_type: MessageBodyType) -> &'static str {
    match body_type {
        MessageBodyType::Html => "html",
        MessageBodyType::Text => "text",
    }
}
