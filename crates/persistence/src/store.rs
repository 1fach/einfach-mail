//! Account-scoped SQLCipher cache operations.

use std::{
    collections::HashSet,
    future::Future,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_sqlite::{
    Client,
    rusqlite::{Connection, OptionalExtension, Result as SqlResult, Transaction, params},
};
use einfach_mail_application::{MailCache, MailCacheError, MailSyncState};
use einfach_mail_domain::{
    AccountId, FolderId, FolderReference, MailFolder, MailMessage, Mailbox, MessageBody,
    MessageBodyType, MessageDeltaChange, MessageDeltaLink, MessageDeltaPage, MessageDeltaProgress,
    MessageId, MessageMetadata, MessageOrigin, ReadState,
};
use thiserror::Error;

use crate::{schema, security};

pub(super) type CacheResult<T> = Result<T, CacheFailure>;

#[derive(Debug, Error)]
pub(super) enum CacheFailure {
    #[error("the database operation failed")]
    Unavailable,
    #[error("the database value is invalid")]
    InvalidData,
    #[error("the message is not cached")]
    MessageNotCached,
}

impl From<async_sqlite::Error> for CacheFailure {
    fn from(_: async_sqlite::Error) -> Self {
        Self::Unavailable
    }
}

impl From<async_sqlite::rusqlite::Error> for CacheFailure {
    fn from(_: async_sqlite::rusqlite::Error) -> Self {
        Self::Unavailable
    }
}

impl CacheFailure {
    pub(super) fn public(self) -> MailCacheError {
        match self {
            Self::Unavailable => MailCacheError::Unavailable,
            Self::InvalidData => MailCacheError::InvalidData,
            Self::MessageNotCached => MailCacheError::MessageNotCached,
        }
    }
}

/// One account's encrypted SQLCipher database and operating-system key reference.
#[derive(Clone)]
pub struct SqliteMailCache {
    account_id: AccountId,
    client: Client,
    database_path: std::path::PathBuf,
    lock_path: std::path::PathBuf,
    credential_account: String,
}

impl SqliteMailCache {
    /// Opens or creates the account's encrypted cache under the Linux data directory.
    ///
    /// A new 32-byte key is generated only when neither the database nor its
    /// sidecar files exist and Secret Service has no saved key.
    ///
    /// # Errors
    ///
    /// Returns a cache error when paths, Secret Service, SQLCipher, or migrations fail.
    pub async fn open_for_account(account_id: AccountId) -> Result<Self, MailCacheError> {
        let opened = security::open_database(&account_id).await?;
        Ok(Self {
            account_id,
            client: opened.client,
            database_path: opened.database_path,
            lock_path: opened.lock_path,
            credential_account: opened.credential_account,
        })
    }

    /// Removes an account's cache files and key when no cache handle is open.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the cache path or credential service cannot be updated.
    pub fn remove_for_account(account_id: &AccountId) -> Result<(), MailCacheError> {
        security::remove_account_without_open_client(account_id)
    }

    #[cfg(test)]
    pub(super) async fn open_with_test_key(
        database_path: std::path::PathBuf,
        account_id: AccountId,
        key_byte: u8,
    ) -> Self {
        let client = security::open_test_database(&database_path, key_byte)
            .await
            .expect("synthetic encrypted cache opens");
        let lock_path = database_path.with_extension("lock");
        Self {
            account_id,
            client,
            database_path,
            lock_path,
            credential_account: "synthetic-cache-key".to_owned(),
        }
    }

    #[cfg(test)]
    pub(super) async fn close_for_tests(&self) {
        self.client
            .close()
            .await
            .expect("synthetic encrypted cache connection closes");
    }

    fn check_account(&self, account_id: &AccountId) -> Result<(), MailCacheError> {
        if &self.account_id == account_id {
            Ok(())
        } else {
            Err(MailCacheError::AccountMismatch)
        }
    }

    fn check_folder(&self, folder: &FolderReference) -> Result<(), MailCacheError> {
        self.check_account(folder.account_id())
    }
}

impl MailCache for SqliteMailCache {
    fn replace_folders(
        &self,
        account_id: &AccountId,
        folders: Vec<MailFolder>,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
        let validation = self
            .check_account(account_id)
            .and_then(|()| validate_folder_snapshot(account_id, &folders));
        let account_id = account_id.as_str().to_owned();
        let client = self.client.clone();
        async move {
            validation?;
            client
                .conn_mut_and_then(move |connection| {
                    let transaction = connection.transaction()?;
                    replace_folder_rows(&transaction, &account_id, folders)?;
                    transaction.commit()?;
                    Ok(())
                })
                .await
                .map_err(CacheFailure::public)
        }
    }

    fn ensure_sync_state(
        &self,
        folder: &FolderReference,
        initial_cutoff: SystemTime,
    ) -> impl Future<Output = Result<MailSyncState, MailCacheError>> + Send + '_ {
        let validation = self
            .check_folder(folder)
            .and_then(|()| timestamp_parts(initial_cutoff));
        let folder = folder.clone();
        let client = self.client.clone();
        async move {
            let (cutoff_seconds, cutoff_nanoseconds) = validation?;
            client
                .conn_mut_and_then(move |connection| {
                    let transaction = connection.transaction()?;
                    if !folder_exists(&transaction, &folder)? {
                        return Err(CacheFailure::InvalidData);
                    }
                    ensure_account_scope_row(
                        &transaction,
                        &folder,
                        cutoff_seconds,
                        cutoff_nanoseconds,
                    )?;
                    transaction.execute(
                        "INSERT OR IGNORE INTO mail_sync_state (
                            account_id, folder_id
                         ) VALUES (?1, ?2)",
                        params![folder.account_id().as_str(), folder.folder_id().as_str()],
                    )?;
                    let state =
                        read_sync_state(&transaction, &folder)?.ok_or(CacheFailure::InvalidData)?;
                    transaction.commit()?;
                    Ok(state)
                })
                .await
                .map_err(CacheFailure::public)
        }
    }

    fn ensure_account_sync_scope(
        &self,
        account_id: &AccountId,
        initial_cutoff: SystemTime,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
        let validation = self
            .check_account(account_id)
            .and_then(|()| timestamp_parts(initial_cutoff));
        let account_id = account_id.as_str().to_owned();
        let client = self.client.clone();
        async move {
            let (cutoff_seconds, cutoff_nanoseconds) = validation?;
            client
                .conn_mut_and_then(move |connection| {
                    let transaction = connection.transaction()?;
                    transaction.execute(
                        "INSERT OR IGNORE INTO mail_account_sync_scope (
                            account_id, query_version, cutoff_seconds, cutoff_nanoseconds
                         ) VALUES (?1, ?2, ?3, ?4)",
                        params![
                            account_id,
                            schema::MESSAGE_QUERY_VERSION,
                            cutoff_seconds,
                            cutoff_nanoseconds
                        ],
                    )?;
                    let query_version: i64 = transaction.query_row(
                        "SELECT query_version FROM mail_account_sync_scope WHERE account_id = ?1",
                        [account_id.as_str()],
                        |row| row.get(0),
                    )?;
                    if query_version != schema::MESSAGE_QUERY_VERSION {
                        return Err(CacheFailure::InvalidData);
                    }
                    transaction.commit()?;
                    Ok(())
                })
                .await
                .map_err(CacheFailure::public)
        }
    }

    fn load_sync_state(
        &self,
        folder: &FolderReference,
    ) -> impl Future<Output = Result<Option<MailSyncState>, MailCacheError>> + Send + '_ {
        let validation = self.check_folder(folder);
        let folder = folder.clone();
        let client = self.client.clone();
        async move {
            validation?;
            client
                .conn_and_then(move |connection| read_sync_state(connection, &folder))
                .await
                .map_err(CacheFailure::public)
        }
    }

    fn commit_delta_page(
        &self,
        folder: &FolderReference,
        page: MessageDeltaPage,
        completed_at: SystemTime,
    ) -> impl Future<Output = Result<MailSyncState, MailCacheError>> + Send + '_ {
        let validation = self
            .check_folder(folder)
            .and_then(|()| timestamp_parts(completed_at));
        let folder = folder.clone();
        let client = self.client.clone();
        async move {
            let completed_at = validation?;
            let (changes, progress) = page.into_parts();
            client
                .conn_mut_and_then(move |connection| {
                    let transaction = connection.transaction()?;
                    let state =
                        read_sync_state(&transaction, &folder)?.ok_or(CacheFailure::InvalidData)?;
                    for change in changes {
                        apply_delta_change(&transaction, &folder, state.cutoff(), change)?;
                    }
                    update_sync_progress(&transaction, &folder, progress, completed_at)?;
                    transaction.commit()?;
                    read_sync_state(connection, &folder)?.ok_or(CacheFailure::InvalidData)
                })
                .await
                .map_err(CacheFailure::public)
        }
    }

    fn list_folders(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<Vec<MailFolder>, MailCacheError>> + Send + '_ {
        let validation = self.check_account(account_id);
        let account_id = account_id.as_str().to_owned();
        let client = self.client.clone();
        async move {
            validation?;
            client
                .conn_and_then(move |connection| read_folders(connection, &account_id))
                .await
                .map_err(CacheFailure::public)
        }
    }

    fn list_messages(
        &self,
        folder: &FolderReference,
    ) -> impl Future<Output = Result<Vec<MailMessage>, MailCacheError>> + Send + '_ {
        let validation = self.check_folder(folder);
        let folder = folder.clone();
        let client = self.client.clone();
        async move {
            validation?;
            client
                .conn_and_then(move |connection| read_messages(connection, &folder))
                .await
                .map_err(CacheFailure::public)
        }
    }

    fn read_message(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
    ) -> impl Future<Output = Result<Option<(MailMessage, Option<MessageBody>)>, MailCacheError>>
    + Send
    + '_ {
        let validation = self.check_folder(folder);
        let folder = folder.clone();
        let message_id = message_id.clone();
        let client = self.client.clone();
        async move {
            validation?;
            client
                .conn_and_then(move |connection| {
                    read_cached_message(connection, &folder, &message_id)
                })
                .await
                .map_err(CacheFailure::public)
        }
    }

    fn store_message_body(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
        body: MessageBody,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
        let validation = self.check_folder(folder);
        let folder = folder.clone();
        let message_id = message_id.clone();
        let body_type = body_type_name(body.content_type());
        let content = body.content().to_owned();
        let client = self.client.clone();
        async move {
            validation?;
            client
                .conn_mut_and_then(move |connection| {
                    let changed = connection.execute(
                        "UPDATE mail_messages
                         SET body_type = ?4, body_content = ?5
                         WHERE account_id = ?1 AND folder_id = ?2 AND message_id = ?3",
                        params![
                            folder.account_id().as_str(),
                            folder.folder_id().as_str(),
                            message_id.as_str(),
                            body_type,
                            content
                        ],
                    )?;
                    if changed == 0 {
                        return Err(CacheFailure::MessageNotCached);
                    }
                    Ok(())
                })
                .await
                .map_err(CacheFailure::public)
        }
    }

    fn remove_account(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
        let validation = self.check_account(account_id);
        let client = self.client.clone();
        let lock_path = self.lock_path.clone();
        let database_path = self.database_path.clone();
        let credential_account = self.credential_account.clone();
        async move {
            validation?;
            let _lock = security::lock_account_directory(&lock_path)?;
            security::delete_database_files(&client, &database_path).await?;
            security::remove_credential(&credential_account)
        }
    }
}

fn validate_folder_snapshot(
    account_id: &AccountId,
    folders: &[MailFolder],
) -> Result<(), MailCacheError> {
    let mut folder_ids = HashSet::with_capacity(folders.len());
    for folder in folders {
        if folder.reference().account_id() != account_id
            || !folder_ids.insert(folder.reference().folder_id().as_str())
        {
            return Err(MailCacheError::InvalidData);
        }
    }
    for folder in folders {
        if folder
            .parent()
            .is_some_and(|parent| !folder_ids.contains(parent.folder_id().as_str()))
        {
            return Err(MailCacheError::InvalidData);
        }
    }
    Ok(())
}

fn replace_folder_rows(
    transaction: &Transaction<'_>,
    account_id: &str,
    folders: Vec<MailFolder>,
) -> CacheResult<()> {
    transaction.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS seen_mail_folders (
            folder_id TEXT PRIMARY KEY
        );
        DELETE FROM seen_mail_folders;",
    )?;
    for folder in folders {
        insert_folder(transaction, account_id, folder)?;
    }
    transaction.execute(
        "DELETE FROM mail_folders
         WHERE account_id = ?1
           AND folder_id NOT IN (SELECT folder_id FROM seen_mail_folders)",
        [account_id],
    )?;
    Ok(())
}

fn insert_folder(
    transaction: &Transaction<'_>,
    account_id: &str,
    folder: MailFolder,
) -> CacheResult<()> {
    let folder_id = folder.reference().folder_id().as_str();
    let parent_id = folder.parent().map(|parent| parent.folder_id().as_str());
    transaction.execute(
        "INSERT INTO mail_folders (account_id, folder_id, parent_folder_id, display_name)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (account_id, folder_id) DO UPDATE SET
            parent_folder_id = excluded.parent_folder_id,
            display_name = excluded.display_name",
        params![account_id, folder_id, parent_id, folder.display_name()],
    )?;
    transaction.execute(
        "INSERT INTO seen_mail_folders (folder_id) VALUES (?1)",
        [folder_id],
    )?;
    Ok(())
}

fn read_folders(connection: &Connection, account_id: &str) -> CacheResult<Vec<MailFolder>> {
    let mut statement = connection.prepare(
        "SELECT folder_id, parent_folder_id, display_name
         FROM mail_folders WHERE account_id = ?1 ORDER BY display_name, folder_id",
    )?;
    let rows = statement.query_map([account_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let values = rows.collect::<Result<Vec<_>, _>>()?;
    values
        .into_iter()
        .map(|(folder_id, parent_id, display_name)| {
            let reference = FolderReference::new(
                AccountId::try_from(account_id.to_owned())
                    .map_err(|_| CacheFailure::InvalidData)?,
                FolderId::try_from(folder_id).map_err(|_| CacheFailure::InvalidData)?,
            );
            let parent = parent_id
                .map(|parent| {
                    FolderId::try_from(parent)
                        .map(|id| FolderReference::new(reference.account_id().clone(), id))
                        .map_err(|_| CacheFailure::InvalidData)
                })
                .transpose()?;
            MailFolder::new(reference, parent, display_name).map_err(|_| CacheFailure::InvalidData)
        })
        .collect()
}

fn read_messages(
    connection: &Connection,
    folder: &FolderReference,
) -> CacheResult<Vec<MailMessage>> {
    if !folder_exists(connection, folder)? {
        return Err(CacheFailure::InvalidData);
    }
    let mut statement = connection.prepare(
        "SELECT message_id, subject, from_display_name, from_address,
                sender_display_name, sender_address, received_seconds,
                received_nanoseconds, is_read
         FROM mail_messages
         WHERE account_id = ?1 AND folder_id = ?2
         ORDER BY received_seconds DESC, received_nanoseconds DESC, message_id ASC",
    )?;
    let rows = statement.query_map(
        params![folder.account_id().as_str(), folder.folder_id().as_str()],
        raw_message_from_row,
    )?;
    rows.map(|row| {
        let raw = row?;
        decode_message(folder, raw)
    })
    .collect()
}

fn folder_exists(connection: &Connection, folder: &FolderReference) -> CacheResult<bool> {
    let exists = connection
        .query_row(
            "SELECT 1 FROM mail_folders WHERE account_id = ?1 AND folder_id = ?2",
            params![folder.account_id().as_str(), folder.folder_id().as_str()],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    Ok(exists)
}

fn read_cached_message(
    connection: &Connection,
    folder: &FolderReference,
    message_id: &MessageId,
) -> CacheResult<Option<(MailMessage, Option<MessageBody>)>> {
    let raw = connection
        .query_row(
            "SELECT message_id, subject, from_display_name, from_address,
                    sender_display_name, sender_address, received_seconds,
                    received_nanoseconds, is_read, body_type, body_content
             FROM mail_messages
             WHERE account_id = ?1 AND folder_id = ?2 AND message_id = ?3",
            params![
                folder.account_id().as_str(),
                folder.folder_id().as_str(),
                message_id.as_str()
            ],
            |row| {
                Ok((
                    raw_message_from_row(row)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                ))
            },
        )
        .optional()?
        .map(|(raw_message, body_type, body_content)| {
            let message = decode_message(folder, raw_message)?;
            let body = decode_body(body_type, body_content)?;
            Ok::<_, CacheFailure>((message, body))
        })
        .transpose()?;
    Ok(raw)
}

fn raw_message_from_row(row: &async_sqlite::rusqlite::Row<'_>) -> SqlResult<RawMessage> {
    Ok(RawMessage {
        message_id: row.get(0)?,
        subject: row.get(1)?,
        from_display_name: row.get(2)?,
        from_address: row.get(3)?,
        sender_display_name: row.get(4)?,
        sender_address: row.get(5)?,
        received_seconds: row.get(6)?,
        received_nanoseconds: row.get(7)?,
        is_read: row.get(8)?,
    })
}

struct RawMessage {
    message_id: String,
    subject: Option<String>,
    from_display_name: Option<String>,
    from_address: Option<String>,
    sender_display_name: Option<String>,
    sender_address: Option<String>,
    received_seconds: i64,
    received_nanoseconds: i64,
    is_read: bool,
}

fn decode_message(folder: &FolderReference, raw: RawMessage) -> CacheResult<MailMessage> {
    let id = MessageId::try_from(raw.message_id).map_err(|_| CacheFailure::InvalidData)?;
    let received_at = timestamp_from_parts(raw.received_seconds, raw.received_nanoseconds)?;
    let from = decode_mailbox(raw.from_display_name, raw.from_address)?;
    let sender = decode_mailbox(raw.sender_display_name, raw.sender_address)?;
    let read_state = if raw.is_read {
        ReadState::Read
    } else {
        ReadState::Unread
    };
    let metadata = MessageMetadata::new(
        raw.subject,
        MessageOrigin::new(from, sender),
        received_at,
        read_state,
    );
    Ok(MailMessage::new(folder.clone(), id, metadata))
}

fn decode_mailbox(
    display_name: Option<String>,
    address: Option<String>,
) -> CacheResult<Option<Mailbox>> {
    match (display_name, address) {
        (None, None) => Ok(None),
        (display_name, Some(address)) => Mailbox::try_new(address, display_name)
            .map(Some)
            .map_err(|_| CacheFailure::InvalidData),
        (Some(_), None) => Err(CacheFailure::InvalidData),
    }
}

fn decode_body(
    body_type: Option<String>,
    body_content: Option<String>,
) -> CacheResult<Option<MessageBody>> {
    match (body_type.as_deref(), body_content) {
        (None, None) => Ok(None),
        (Some("html"), Some(content)) => Ok(Some(MessageBody::new(MessageBodyType::Html, content))),
        (Some("text"), Some(content)) => Ok(Some(MessageBody::new(MessageBodyType::Text, content))),
        _ => Err(CacheFailure::InvalidData),
    }
}

fn read_sync_state(
    connection: &Connection,
    folder: &FolderReference,
) -> CacheResult<Option<MailSyncState>> {
    let raw = connection
        .query_row(
            "SELECT a.query_version, a.cutoff_seconds, a.cutoff_nanoseconds,
                    s.delta_link, s.next_link, s.last_sync_seconds, s.last_sync_nanoseconds
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
}

fn decode_sync_state(raw: RawSyncState) -> CacheResult<MailSyncState> {
    if raw.query_version != schema::MESSAGE_QUERY_VERSION {
        return Err(CacheFailure::InvalidData);
    }
    let cutoff = timestamp_from_parts(raw.cutoff_seconds, raw.cutoff_nanoseconds)?;
    let last_synchronized_at = match (raw.sync_seconds, raw.sync_nanoseconds) {
        (Some(seconds), Some(nanoseconds)) => Some(timestamp_from_parts(seconds, nanoseconds)?),
        (None, None) => None,
        _ => return Err(CacheFailure::InvalidData),
    };
    MailSyncState::from_checkpoint(
        cutoff,
        raw.delta_link.map(MessageDeltaLink::from_opaque),
        raw.next_link.map(MessageDeltaLink::from_opaque),
        last_synchronized_at,
    )
    .map_err(|_| CacheFailure::InvalidData)
}

fn ensure_account_scope_row(
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

fn apply_delta_change(
    connection: &Connection,
    folder: &FolderReference,
    cutoff: SystemTime,
    change: MessageDeltaChange,
) -> CacheResult<()> {
    match change {
        MessageDeltaChange::Removed { id } => remove_message(connection, folder, &id),
        MessageDeltaChange::Upsert { id, patch } => {
            apply_message_patch(connection, folder, cutoff, id, patch)
        }
    }
}

fn apply_message_patch(
    connection: &Connection,
    folder: &FolderReference,
    cutoff: SystemTime,
    id: MessageId,
    patch: einfach_mail_domain::MessageMetadataPatch,
) -> CacheResult<()> {
    let mut cached = read_cached_metadata(connection, folder, &id)?;
    let Some(message) = cached.as_mut() else {
        if let Some(message) = patch.into_complete_message(folder.clone(), id)
            && message.metadata().received_at() >= &cutoff
        {
            return store_message_metadata(connection, &message);
        }
        return Ok(());
    };
    message.apply_patch(patch);
    if message.metadata().received_at() < &cutoff {
        return remove_message(connection, folder, message.id());
    }
    store_message_metadata(connection, message)
}

fn read_cached_metadata(
    connection: &Connection,
    folder: &FolderReference,
    message_id: &MessageId,
) -> CacheResult<Option<MailMessage>> {
    let raw = connection
        .query_row(
            "SELECT message_id, subject, from_display_name, from_address,
                    sender_display_name, sender_address, received_seconds,
                    received_nanoseconds, is_read
             FROM mail_messages
             WHERE account_id = ?1 AND folder_id = ?2 AND message_id = ?3",
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

fn store_message_metadata(connection: &Connection, message: &MailMessage) -> CacheResult<()> {
    let metadata = message.metadata();
    let (from_name, from_address) = mailbox_fields(metadata.origin().from_mailbox());
    let (sender_name, sender_address) = mailbox_fields(metadata.origin().sender());
    let (received_seconds, received_nanoseconds) =
        timestamp_parts(*metadata.received_at()).map_err(|_| CacheFailure::InvalidData)?;
    connection.execute(
        "INSERT INTO mail_messages (
            account_id, folder_id, message_id, subject,
            from_display_name, from_address, sender_display_name, sender_address,
            received_seconds, received_nanoseconds, is_read
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT (account_id, folder_id, message_id) DO UPDATE SET
            subject = excluded.subject,
            from_display_name = excluded.from_display_name,
            from_address = excluded.from_address,
            sender_display_name = excluded.sender_display_name,
            sender_address = excluded.sender_address,
            received_seconds = excluded.received_seconds,
            received_nanoseconds = excluded.received_nanoseconds,
            is_read = excluded.is_read",
        params![
            message.folder().account_id().as_str(),
            message.folder().folder_id().as_str(),
            message.id().as_str(),
            metadata.subject(),
            from_name,
            from_address,
            sender_name,
            sender_address,
            received_seconds,
            received_nanoseconds,
            matches!(metadata.read_state(), ReadState::Read)
        ],
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

fn update_sync_progress(
    transaction: &Transaction<'_>,
    folder: &FolderReference,
    progress: MessageDeltaProgress,
    completed_at: (i64, i64),
) -> CacheResult<()> {
    match progress {
        MessageDeltaProgress::Next(link) => {
            transaction.execute(
                "UPDATE mail_sync_state SET next_link = ?3
                 WHERE account_id = ?1 AND folder_id = ?2",
                params![
                    folder.account_id().as_str(),
                    folder.folder_id().as_str(),
                    link.as_str()
                ],
            )?;
        }
        MessageDeltaProgress::Complete(link) => {
            transaction.execute(
                "UPDATE mail_sync_state
                 SET delta_link = ?3, next_link = NULL,
                     last_sync_seconds = ?4, last_sync_nanoseconds = ?5
                 WHERE account_id = ?1 AND folder_id = ?2",
                params![
                    folder.account_id().as_str(),
                    folder.folder_id().as_str(),
                    link.as_str(),
                    completed_at.0,
                    completed_at.1
                ],
            )?;
        }
    }
    Ok(())
}

fn body_type_name(body_type: MessageBodyType) -> &'static str {
    match body_type {
        MessageBodyType::Html => "html",
        MessageBodyType::Text => "text",
    }
}

fn timestamp_parts(timestamp: SystemTime) -> Result<(i64, i64), MailCacheError> {
    match timestamp.duration_since(UNIX_EPOCH) {
        Ok(duration) => positive_timestamp_parts(duration),
        Err(error) => negative_timestamp_parts(error.duration()),
    }
}

fn positive_timestamp_parts(duration: Duration) -> Result<(i64, i64), MailCacheError> {
    let seconds = i64::try_from(duration.as_secs()).map_err(|_| MailCacheError::InvalidData)?;
    Ok((seconds, i64::from(duration.subsec_nanos())))
}

fn negative_timestamp_parts(duration: Duration) -> Result<(i64, i64), MailCacheError> {
    let seconds = i64::try_from(duration.as_secs()).map_err(|_| MailCacheError::InvalidData)?;
    let nanos = i64::from(duration.subsec_nanos());
    if nanos == 0 {
        return Ok((-seconds, 0));
    }
    let seconds = seconds
        .checked_neg()
        .and_then(|seconds| seconds.checked_sub(1))
        .ok_or(MailCacheError::InvalidData)?;
    Ok((seconds, 1_000_000_000 - nanos))
}

fn timestamp_from_parts(seconds: i64, nanoseconds: i64) -> CacheResult<SystemTime> {
    if !(0..1_000_000_000).contains(&nanoseconds) {
        return Err(CacheFailure::InvalidData);
    }
    let subsecond = Duration::from_nanos(nanoseconds as u64);
    let timestamp = if seconds >= 0 {
        UNIX_EPOCH.checked_add(Duration::from_secs(seconds as u64) + subsecond)
    } else {
        let distance = Duration::from_secs(seconds.unsigned_abs())
            .checked_sub(subsecond)
            .ok_or(CacheFailure::InvalidData)?;
        UNIX_EPOCH.checked_sub(distance)
    };
    timestamp.ok_or(CacheFailure::InvalidData)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use einfach_mail_application::{MailCache, MailCacheError};
    use einfach_mail_domain::{
        AccountId, FolderId, FolderReference, MailFolder, MessageBodyType, MessageDeltaChange,
        MessageDeltaLink, MessageDeltaPage, MessageDeltaProgress, MessageId, MessageMetadataPatch,
        ReadState,
    };
    use tempfile::TempDir;

    use super::{SqliteMailCache, timestamp_from_parts, timestamp_parts};
    const DATABASE_KEY_BYTE: u8 = 0x51;
    const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

    fn account_id(value: &str) -> AccountId {
        AccountId::try_from(value.to_owned()).expect("synthetic account ID is non-empty")
    }

    fn folder(account_id: &AccountId, value: &str) -> MailFolder {
        MailFolder::new(
            folder_reference(account_id, value),
            None,
            "Synthetic Inbox".to_owned(),
        )
        .expect("synthetic folder is valid")
    }

    fn folder_reference(account_id: &AccountId, value: &str) -> FolderReference {
        FolderReference::new(
            account_id.clone(),
            FolderId::try_from(value.to_owned()).expect("synthetic folder ID is non-empty"),
        )
    }

    fn message_id(value: &str) -> MessageId {
        MessageId::try_from(value.to_owned()).expect("synthetic message ID is non-empty")
    }

    async fn open_test_cache(directory: &TempDir, account_id: AccountId) -> SqliteMailCache {
        let database_path = directory.path().join("mail.sqlite3");
        SqliteMailCache::open_with_test_key(database_path, account_id, DATABASE_KEY_BYTE).await
    }

    fn complete_patch(
        subject: Option<&str>,
        received_at: SystemTime,
        read_state: ReadState,
    ) -> MessageMetadataPatch {
        let mut patch = MessageMetadataPatch::new();
        patch.set_subject(subject.map(str::to_owned));
        patch.set_from(None);
        patch.set_sender(None);
        patch.set_received_at(received_at);
        patch.set_read_state(read_state);
        patch
    }

    #[test]
    fn page_data_and_opaque_cursor_commit_atomically_and_resume_after_reopen() {
        smol::block_on(async {
            let directory = TempDir::new().expect("temporary directory is created");
            let account = account_id("synthetic-account");
            let cached_folder = folder(&account, "folder-inbox");
            let folder_reference = cached_folder.reference().clone();
            let cutoff = UNIX_EPOCH + Duration::from_secs(100);
            let cache = open_test_cache(&directory, account.clone()).await;
            cache
                .replace_folders(&account, vec![cached_folder.clone()])
                .await
                .expect("synthetic folder is cached");

            let initial_state = cache
                .ensure_sync_state(&folder_reference, cutoff)
                .await
                .expect("initial cutoff and cursor state are saved");
            assert_eq!(initial_state.cutoff(), cutoff);
            let request_cutoff = cutoff - Duration::from_secs(1_000);
            let resumed_state = cache
                .ensure_sync_state(&folder_reference, request_cutoff)
                .await
                .expect("existing cursor state is reused");
            assert_eq!(resumed_state.cutoff(), cutoff);

            let page = synthetic_initial_page(cutoff);
            let link = MessageDeltaLink::from_opaque(
                "https://graph.microsoft.com/v1.0/messages?$skiptoken=synthetic-next".to_owned(),
            );
            let page = MessageDeltaPage::new(page, MessageDeltaProgress::Next(link.clone()));
            let first_commit = cache
                .commit_delta_page(&folder_reference, page.clone(), cutoff)
                .await
                .expect("first page commits with its resumable cursor");
            let repeated_commit = cache
                .commit_delta_page(&folder_reference, page, cutoff)
                .await
                .expect("replaying an uncheckpointed page is safe");
            assert_eq!(first_commit, repeated_commit);
            assert_eq!(repeated_commit.next_link(), Some(&link));
            assert!(repeated_commit.delta_link().is_none());
            assert!(repeated_commit.last_synchronized_at().is_none());
            assert_eq!(
                cache
                    .list_messages(&folder_reference)
                    .await
                    .expect("cached messages are readable")
                    .len(),
                1
            );

            let message = message_id("message-at-cutoff");
            cache
                .store_message_body(
                    &folder_reference,
                    &message,
                    einfach_mail_domain::MessageBody::new(
                        MessageBodyType::Html,
                        "<p>Synthetic offline body</p>".to_owned(),
                    ),
                )
                .await
                .expect("opened synthetic body is saved");
            cache
                .client
                .close()
                .await
                .expect("synthetic cache connection closes");

            let reopened = open_test_cache(&directory, account).await;
            let persisted_state = reopened
                .load_sync_state(&folder_reference)
                .await
                .expect("stored cursor remains readable")
                .expect("folder cursor exists");
            assert_eq!(persisted_state.next_link(), Some(&link));
            let (stored_message, stored_body) = reopened
                .read_message(&folder_reference, &message)
                .await
                .expect("cached message is readable after restart")
                .expect("message remains in the folder cache");
            assert_eq!(stored_message.id(), &message);
            assert_eq!(
                stored_body
                    .as_ref()
                    .map(einfach_mail_domain::MessageBody::content),
                Some("<p>Synthetic offline body</p>")
            );

            let database_bytes = std::fs::read(&reopened.database_path)
                .expect("synthetic encrypted database is readable as bytes");
            assert_ne!(&database_bytes[..SQLITE_HEADER.len()], SQLITE_HEADER);
            reopened.close_for_tests().await;
        });
    }

    fn synthetic_initial_page(cutoff: SystemTime) -> Vec<MessageDeltaChange> {
        let exact_cutoff = complete_patch(Some("At cutoff"), cutoff, ReadState::Unread);
        let old_message = complete_patch(
            Some("Before cutoff"),
            cutoff - Duration::from_nanos(1),
            ReadState::Read,
        );
        let mut incomplete_patch = MessageMetadataPatch::new();
        incomplete_patch.set_subject(Some("Incomplete metadata".to_owned()));
        vec![
            MessageDeltaChange::Upsert {
                id: message_id("message-at-cutoff"),
                patch: exact_cutoff,
            },
            MessageDeltaChange::Upsert {
                id: message_id("message-before-cutoff"),
                patch: old_message,
            },
            MessageDeltaChange::Upsert {
                id: message_id("message-incomplete"),
                patch: incomplete_patch,
            },
        ]
    }

    #[test]
    fn account_scope_and_folder_snapshot_are_enforced() {
        smol::block_on(async {
            let directory = TempDir::new().expect("temporary directory is created");
            let account = account_id("synthetic-account");
            let other_account = account_id("other-account");
            let cache = open_test_cache(&directory, account.clone()).await;
            let foreign_folder = folder_reference(&other_account, "folder-inbox");

            assert_eq!(
                cache.list_messages(&foreign_folder).await,
                Err(MailCacheError::AccountMismatch)
            );
            let folders = vec![folder(&account, "folder-inbox")];
            cache
                .replace_folders(&account, folders.clone())
                .await
                .expect("synthetic folder snapshot is valid");
            assert_eq!(
                cache.replace_folders(&other_account, folders).await,
                Err(MailCacheError::AccountMismatch)
            );
            cache.close_for_tests().await;
        });
    }

    #[test]
    fn account_cutoff_survives_an_empty_folder_snapshot_and_later_folders() {
        smol::block_on(async {
            let directory = TempDir::new().expect("temporary directory is created");
            let account = account_id("synthetic-account");
            let cutoff = UNIX_EPOCH + Duration::from_secs(25_000);
            let cache = open_test_cache(&directory, account.clone()).await;
            cache
                .ensure_account_sync_scope(&account, cutoff)
                .await
                .expect("empty initial account scope is saved");
            cache
                .replace_folders(&account, Vec::new())
                .await
                .expect("empty folder snapshot is valid");
            cache.close_for_tests().await;

            let reopened = open_test_cache(&directory, account.clone()).await;
            let later_folder = folder(&account, "folder-added-later");
            reopened
                .replace_folders(&account, vec![later_folder.clone()])
                .await
                .expect("a later visible folder is saved");
            let later_state = reopened
                .ensure_sync_state(
                    later_folder.reference(),
                    cutoff + Duration::from_secs(30 * 24 * 60 * 60),
                )
                .await
                .expect("a later folder gets a cursor under the account's original scope");

            assert_eq!(later_state.cutoff(), cutoff);
            reopened.close_for_tests().await;
        });
    }

    #[test]
    fn system_time_parts_round_trip_around_the_unix_epoch() {
        for timestamp in [
            UNIX_EPOCH - Duration::from_nanos(1),
            UNIX_EPOCH,
            UNIX_EPOCH + Duration::from_secs(12) + Duration::from_nanos(345),
        ] {
            let parts = timestamp_parts(timestamp).expect("synthetic time fits SQLite");
            assert_eq!(
                timestamp_from_parts(parts.0, parts.1).expect("stored synthetic time is valid"),
                timestamp
            );
        }
    }
}
