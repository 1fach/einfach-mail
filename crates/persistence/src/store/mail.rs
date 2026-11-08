use std::{collections::HashSet, future::Future, time::SystemTime};

#[cfg(test)]
use std::sync::{Arc, atomic::Ordering};

use async_sqlite::rusqlite::{
    Connection, OptionalExtension, Result as SqlResult, Transaction, TransactionBehavior, params,
};
use einfach_mail_application::{MailCache, MailCacheError, MailSyncState};
use einfach_mail_domain::{
    AccountId, FolderId, FolderReference, FollowUpFlagStatus, MailFolder, MailMessage,
    MailRecipient, MailReplySource, MailSendOperationId, MailSendTarget, MailSubject, Mailbox,
    MessageBody, MessageBodyType, MessageDeltaChange, MessageDeltaLink, MessageDeltaPage,
    MessageDeltaProgress, MessageId, MessageMetadata, MessageOrigin, PendingMailSend, ReadState,
};
use thiserror::Error;
use uuid::Uuid;

use super::{
    CacheFailure, CacheResult, SqliteMailCache,
    system_time::{system_time_from_parts, system_time_parts},
};
use crate::schema;

type MailCacheResult<T> = Result<T, MailCacheFailure>;

#[derive(Debug, Error)]
enum MailCacheFailure {
    #[error(transparent)]
    Store(#[from] CacheFailure),
    #[error("the message is not cached")]
    MessageNotCached,
    #[error("the folder synchronization checkpoint changed")]
    SyncStateChanged,
    #[error("an unresolved send marker already exists")]
    SendAlreadyPending,
    #[error("the pending send belongs to another operation")]
    PendingSendMismatch,
    #[error("the pending send marker does not exist")]
    PendingSendNotFound,
}

impl MailCacheFailure {
    fn public(self) -> MailCacheError {
        match self {
            Self::Store(error) => error.mail_public(),
            Self::MessageNotCached => MailCacheError::MessageNotCached,
            Self::SyncStateChanged => MailCacheError::SyncStateChanged,
            Self::SendAlreadyPending => MailCacheError::SendAlreadyPending,
            Self::PendingSendMismatch => MailCacheError::PendingSendMismatch,
            Self::PendingSendNotFound => MailCacheError::PendingSendNotFound,
        }
    }
}

impl From<async_sqlite::Error> for MailCacheFailure {
    fn from(error: async_sqlite::Error) -> Self {
        Self::Store(CacheFailure::from(error))
    }
}

impl From<async_sqlite::rusqlite::Error> for MailCacheFailure {
    fn from(error: async_sqlite::rusqlite::Error) -> Self {
        Self::Store(CacheFailure::from(error))
    }
}

impl SqliteMailCache {
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
                .map_err(CacheFailure::mail_public)
        }
    }

    fn ensure_sync_state(
        &self,
        folder: &FolderReference,
        initial_cutoff: SystemTime,
    ) -> impl Future<Output = Result<MailSyncState, MailCacheError>> + Send + '_ {
        let validation = self
            .check_folder(folder)
            .and_then(|()| system_time_parts(initial_cutoff).map_err(CacheFailure::mail_public));
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
                .map_err(CacheFailure::mail_public)
        }
    }

    fn ensure_account_sync_scope(
        &self,
        account_id: &AccountId,
        initial_cutoff: SystemTime,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
        let validation = self
            .check_account(account_id)
            .and_then(|()| system_time_parts(initial_cutoff).map_err(CacheFailure::mail_public));
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
                .map_err(CacheFailure::mail_public)
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
                .map_err(CacheFailure::mail_public)
        }
    }

    fn commit_delta_page(
        &self,
        folder: &FolderReference,
        expected_state: &MailSyncState,
        page: MessageDeltaPage,
        completed_at: SystemTime,
    ) -> impl Future<Output = Result<MailSyncState, MailCacheError>> + Send + '_ {
        let validation = self
            .check_folder(folder)
            .and_then(|()| system_time_parts(completed_at).map_err(CacheFailure::mail_public));
        let folder = folder.clone();
        let expected_state = expected_state.clone();
        let client = self.client.clone();
        async move {
            let completed_at = validation?;
            let (changes, progress) = page.into_parts();
            client
                .conn_mut_and_then(move |connection| {
                    let transaction = begin_checkpoint_write(connection)?;
                    let state =
                        read_sync_state(&transaction, &folder)?.ok_or(CacheFailure::InvalidData)?;
                    if state != expected_state {
                        return Err(MailCacheFailure::SyncStateChanged);
                    }
                    let full_sync_generation = state
                        .full_sync_generation()
                        .map(i64::try_from)
                        .transpose()
                        .map_err(|_| CacheFailure::InvalidData)?;
                    let checkpoint_revision = state
                        .checkpoint_revision()
                        .checked_add(1)
                        .and_then(|revision| i64::try_from(revision).ok())
                        .ok_or(CacheFailure::InvalidData)?;
                    for change in changes {
                        apply_delta_change(
                            &transaction,
                            &folder,
                            state.cutoff(),
                            full_sync_generation,
                            change,
                        )?;
                    }
                    update_sync_progress(
                        &transaction,
                        &folder,
                        progress,
                        completed_at,
                        checkpoint_revision,
                    )?;
                    transaction.commit()?;
                    read_sync_state(connection, &folder)?.ok_or(CacheFailure::InvalidData.into())
                })
                .await
                .map_err(MailCacheFailure::public)
        }
    }

    fn begin_full_resync(
        &self,
        folder: &FolderReference,
        expected_state: &MailSyncState,
        reset_link: MessageDeltaLink,
    ) -> impl Future<Output = Result<MailSyncState, MailCacheError>> + Send + '_ {
        let validation = self.check_folder(folder);
        let folder = folder.clone();
        let expected_state = expected_state.clone();
        let reset_link = reset_link.as_str().to_owned();
        let client = self.client.clone();
        async move {
            validation?;
            client
                .conn_mut_and_then(move |connection| {
                    begin_full_resync_transaction(connection, &folder, &expected_state, &reset_link)
                })
                .await
                .map_err(MailCacheFailure::public)
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
                .map_err(CacheFailure::mail_public)
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
                .map_err(CacheFailure::mail_public)
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
                .map_err(CacheFailure::mail_public)
        }
    }

    fn message_read_state(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
    ) -> impl Future<Output = Result<Option<ReadState>, MailCacheError>> + Send + '_ {
        let validation = self.check_folder(folder);
        let folder = folder.clone();
        let message_id = message_id.clone();
        let client = self.client.clone();
        async move {
            validation?;
            client
                .conn_and_then(move |connection| {
                    let is_read = connection
                        .query_row(
                            "SELECT is_read FROM mail_messages
                             WHERE account_id = ?1 AND folder_id = ?2 AND message_id = ?3",
                            params![
                                folder.account_id().as_str(),
                                folder.folder_id().as_str(),
                                message_id.as_str()
                            ],
                            |row| row.get::<_, bool>(0),
                        )
                        .optional()?;
                    Ok(is_read.map(read_state_from_bool))
                })
                .await
                .map_err(CacheFailure::mail_public)
        }
    }

    fn set_message_read_state(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
        read_state: ReadState,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
        let validation = self.check_folder(folder);
        let folder = folder.clone();
        let message_id = message_id.clone();
        let is_read = read_state == ReadState::Read;
        let client = self.client.clone();
        async move {
            validation?;
            client
                .conn_mut_and_then(move |connection| {
                    let transaction = connection.transaction()?;
                    let changed = transaction.execute(
                        "UPDATE mail_messages SET is_read = ?4
                         WHERE account_id = ?1 AND folder_id = ?2 AND message_id = ?3",
                        params![
                            folder.account_id().as_str(),
                            folder.folder_id().as_str(),
                            message_id.as_str(),
                            is_read
                        ],
                    )?;
                    if changed == 0 {
                        return Err(MailCacheFailure::MessageNotCached);
                    }
                    transaction.execute(
                        "DELETE FROM mail_folder_write_sync_requirement
                         WHERE account_id = ?1 AND folder_id = ?2",
                        params![folder.account_id().as_str(), folder.folder_id().as_str()],
                    )?;
                    transaction.commit()?;
                    Ok(())
                })
                .await
                .map_err(MailCacheFailure::public)
        }
    }

    fn folder_write_sync_required(
        &self,
        folder: &FolderReference,
    ) -> impl Future<Output = Result<bool, MailCacheError>> + Send + '_ {
        let validation = self.check_folder(folder);
        let folder = folder.clone();
        let client = self.client.clone();
        async move {
            validation?;
            client
                .conn_and_then(move |connection| {
                    if !folder_exists(connection, &folder)? {
                        return Err(CacheFailure::InvalidData);
                    }
                    let required = connection.query_row(
                        "SELECT EXISTS (
                            SELECT 1 FROM mail_folder_write_sync_requirement
                            WHERE account_id = ?1 AND folder_id = ?2
                        )",
                        params![folder.account_id().as_str(), folder.folder_id().as_str()],
                        |row| row.get(0),
                    )?;
                    Ok(required)
                })
                .await
                .map_err(CacheFailure::mail_public)
        }
    }

    fn require_folder_write_sync(
        &self,
        folder: &FolderReference,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
        let validation = self.check_folder(folder);
        let folder = folder.clone();
        let client = self.client.clone();
        async move {
            validation?;
            client
                .conn_mut_and_then(move |connection| {
                    let transaction = connection.transaction()?;
                    if !folder_exists(&transaction, &folder)? {
                        return Err(CacheFailure::InvalidData);
                    }
                    transaction.execute(
                        "INSERT OR IGNORE INTO mail_folder_write_sync_requirement (
                            account_id, folder_id
                        ) VALUES (?1, ?2)",
                        params![folder.account_id().as_str(), folder.folder_id().as_str()],
                    )?;
                    transaction.commit()?;
                    Ok(())
                })
                .await
                .map_err(CacheFailure::mail_public)
        }
    }

    fn clear_folder_write_sync_requirement(
        &self,
        folder: &FolderReference,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
        let validation = self.check_folder(folder);
        let folder = folder.clone();
        let client = self.client.clone();
        async move {
            validation?;
            client
                .conn_mut_and_then(move |connection| {
                    connection.execute(
                        "DELETE FROM mail_folder_write_sync_requirement
                         WHERE account_id = ?1 AND folder_id = ?2",
                        params![folder.account_id().as_str(), folder.folder_id().as_str()],
                    )?;
                    Ok(())
                })
                .await
                .map_err(CacheFailure::mail_public)
        }
    }

    fn set_message_follow_up_flag_status(
        &self,
        folder: &FolderReference,
        message_id: &MessageId,
        status: FollowUpFlagStatus,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
        let validation = self.check_folder(folder);
        let folder = folder.clone();
        let message_id = message_id.clone();
        let flag_status = follow_up_flag_status_name(status);
        let client = self.client.clone();
        async move {
            validation?;
            client
                .conn_mut_and_then(move |connection| {
                    let transaction = connection.transaction()?;
                    let changed = transaction.execute(
                        "UPDATE mail_messages SET flag_status = ?4
                         WHERE account_id = ?1 AND folder_id = ?2 AND message_id = ?3",
                        params![
                            folder.account_id().as_str(),
                            folder.folder_id().as_str(),
                            message_id.as_str(),
                            flag_status
                        ],
                    )?;
                    if changed == 0 {
                        return Err(MailCacheFailure::MessageNotCached);
                    }
                    transaction.execute(
                        "DELETE FROM mail_folder_write_sync_requirement
                         WHERE account_id = ?1 AND folder_id = ?2",
                        params![folder.account_id().as_str(), folder.folder_id().as_str()],
                    )?;
                    transaction.commit()?;
                    Ok(())
                })
                .await
                .map_err(MailCacheFailure::public)
        }
    }

    fn move_message(
        &self,
        source_folder: &FolderReference,
        destination_folder: &FolderReference,
        message_id: &MessageId,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
        let validation = self
            .check_folder(source_folder)
            .and_then(|()| self.check_folder(destination_folder));
        let source_folder = source_folder.clone();
        let destination_folder = destination_folder.clone();
        let message_id = message_id.clone();
        let client = self.client.clone();
        async move {
            validation?;
            if source_folder == destination_folder {
                return Err(MailCacheError::InvalidData);
            }
            client
                .conn_mut_and_then(move |connection| {
                    let transaction = connection.transaction()?;
                    move_cached_message(
                        &transaction,
                        &source_folder,
                        &destination_folder,
                        &message_id,
                    )?;
                    transaction.commit()?;
                    Ok(())
                })
                .await
                .map_err(MailCacheFailure::public)
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
                        return Err(MailCacheFailure::MessageNotCached);
                    }
                    Ok(())
                })
                .await
                .map_err(MailCacheFailure::public)
        }
    }

    fn load_pending_send(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<Option<PendingMailSend>, MailCacheError>> + Send + '_ {
        let validation = self.check_account(account_id);
        let account_id = account_id.as_str().to_owned();
        let client = self.client.clone();
        #[cfg(test)]
        let pending_send_read_gate = Arc::clone(&self.pending_send_read_gate);
        async move {
            validation?;
            #[cfg(test)]
            let gate = pending_send_read_gate
                .lock()
                .expect("synthetic pending-send read gate mutex is healthy")
                .take();
            #[cfg(test)]
            if let Some((started, release)) = gate {
                let _ = started.send(()).await;
                let _ = release.recv().await;
            }
            client
                .conn_and_then(move |connection| read_pending_send(connection, &account_id))
                .await
                .map_err(CacheFailure::mail_public)
        }
    }

    fn stage_pending_send(
        &self,
        pending: &PendingMailSend,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
        let validation = self.check_account(pending.account_id()).and_then(|()| {
            system_time_parts(pending.created_at()).map_err(CacheFailure::mail_public)
        });
        let pending = pending.clone();
        let client = self.client.clone();
        let database_path = self.database_path.clone();
        let database_identity = self.database_identity;
        async move {
            let (created_seconds, created_nanoseconds) = validation?;
            let account_id = pending.account_id().as_str().to_owned();
            let operation_id = pending.operation_id().as_uuid().to_string();
            let (target_kind, recipient, subject, source_folder_id, source_message_id) =
                match pending.target() {
                    MailSendTarget::NewMessage { recipient, subject } => (
                        "new_message",
                        Some(recipient.as_str().to_owned()),
                        Some(subject.as_str().to_owned()),
                        None,
                        None,
                    ),
                    MailSendTarget::Reply { source, subject } => (
                        "reply",
                        None,
                        subject.as_ref().map(|subject| subject.to_string()),
                        Some(source.folder().folder_id().as_str().to_owned()),
                        Some(source.message_id().as_str().to_owned()),
                    ),
                };
            if !database_identity.matches_path(&database_path)? {
                return Err(MailCacheError::Unavailable);
            }
            client
                .conn_mut_and_then(move |connection| {
                    let changed = connection.execute(
                        "INSERT OR IGNORE INTO mail_pending_send (
                            account_id, operation_id, target_kind, recipient, subject,
                            source_folder_id, source_message_id,
                            created_seconds, created_nanoseconds
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                        params![
                            account_id,
                            operation_id,
                            target_kind,
                            recipient,
                            subject,
                            source_folder_id,
                            source_message_id,
                            created_seconds,
                            created_nanoseconds
                        ],
                    )?;
                    if changed == 0 {
                        return Err(MailCacheFailure::SendAlreadyPending);
                    }
                    Ok(())
                })
                .await
                .map_err(MailCacheFailure::public)
        }
    }

    fn clear_pending_send(
        &self,
        account_id: &AccountId,
        operation_id: MailSendOperationId,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
        let validation = self.check_account(account_id);
        let account_id = account_id.as_str().to_owned();
        let operation_id = operation_id.as_uuid().to_string();
        let client = self.client.clone();
        #[cfg(test)]
        let fail_next_pending_send_clear = Arc::clone(&self.fail_next_pending_send_clear);
        async move {
            validation?;
            #[cfg(test)]
            if fail_next_pending_send_clear.swap(false, Ordering::Relaxed) {
                return Err(MailCacheError::Unavailable);
            }
            client
                .conn_mut_and_then(move |connection| {
                    let transaction = connection.transaction()?;
                    let existing = transaction
                        .query_row(
                            "SELECT operation_id FROM mail_pending_send WHERE account_id = ?1",
                            [account_id.as_str()],
                            |row| row.get::<_, String>(0),
                        )
                        .optional()?;
                    match existing.as_deref() {
                        None => return Err(MailCacheFailure::PendingSendNotFound),
                        Some(existing) if existing != operation_id => {
                            return Err(MailCacheFailure::PendingSendMismatch);
                        }
                        Some(_) => {}
                    }
                    transaction.execute(
                        "DELETE FROM mail_pending_send
                         WHERE account_id = ?1 AND operation_id = ?2",
                        params![account_id, operation_id],
                    )?;
                    transaction.commit()?;
                    Ok(())
                })
                .await
                .map_err(MailCacheFailure::public)
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

struct StoredPendingSend {
    operation_id: String,
    target_kind: String,
    target: StoredPendingSendTarget,
    created_seconds: i64,
    created_nanoseconds: i64,
}

struct StoredPendingSendTarget {
    recipient: Option<String>,
    subject: Option<String>,
    source_folder_id: Option<String>,
    source_message_id: Option<String>,
}

impl StoredPendingSend {
    fn read(connection: &Connection, account_id: &str) -> CacheResult<Option<Self>> {
        connection
            .query_row(
                "SELECT operation_id, target_kind, recipient, subject,
                        source_folder_id, source_message_id,
                        created_seconds, created_nanoseconds
                 FROM mail_pending_send WHERE account_id = ?1",
                [account_id],
                Self::from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    fn from_row(row: &async_sqlite::rusqlite::Row<'_>) -> SqlResult<Self> {
        Ok(Self {
            operation_id: row.get(0)?,
            target_kind: row.get(1)?,
            target: StoredPendingSendTarget {
                recipient: row.get(2)?,
                subject: row.get(3)?,
                source_folder_id: row.get(4)?,
                source_message_id: row.get(5)?,
            },
            created_seconds: row.get(6)?,
            created_nanoseconds: row.get(7)?,
        })
    }

    fn into_domain(self, account_id: &str) -> CacheResult<PendingMailSend> {
        let Self {
            operation_id,
            target_kind,
            target,
            created_seconds,
            created_nanoseconds,
        } = self;
        let account_id =
            AccountId::try_from(account_id.to_owned()).map_err(|_| CacheFailure::InvalidData)?;
        let operation_id = Uuid::parse_str(&operation_id)
            .map(MailSendOperationId::from_uuid)
            .map_err(|_| CacheFailure::InvalidData)?;
        let created_at = system_time_from_parts(created_seconds, created_nanoseconds)?;
        match target_kind.as_str() {
            "new_message" => target.into_new_message(account_id, operation_id, created_at),
            "reply" => target.into_reply(account_id, operation_id, created_at),
            _ => Err(CacheFailure::InvalidData),
        }
    }
}

impl StoredPendingSendTarget {
    fn into_new_message(
        self,
        account_id: AccountId,
        operation_id: MailSendOperationId,
        created_at: SystemTime,
    ) -> CacheResult<PendingMailSend> {
        let (recipient, subject) = self.new_message_values()?;
        Ok(PendingMailSend::for_new_message(
            account_id,
            operation_id,
            recipient,
            subject,
            created_at,
        ))
    }

    fn new_message_values(self) -> CacheResult<(MailRecipient, MailSubject)> {
        if self.source_folder_id.is_some() || self.source_message_id.is_some() {
            return Err(CacheFailure::InvalidData);
        }
        let recipient = self
            .recipient
            .ok_or(CacheFailure::InvalidData)?
            .parse::<MailRecipient>()
            .map_err(|_| CacheFailure::InvalidData)?;
        let subject = self
            .subject
            .ok_or(CacheFailure::InvalidData)?
            .parse::<MailSubject>()
            .map_err(|_| CacheFailure::InvalidData)?;
        Ok((recipient, subject))
    }

    fn into_reply(
        self,
        account_id: AccountId,
        operation_id: MailSendOperationId,
        created_at: SystemTime,
    ) -> CacheResult<PendingMailSend> {
        let (source, subject) = self.reply_values(account_id)?;
        Ok(PendingMailSend::for_reply(
            source,
            operation_id,
            subject,
            created_at,
        ))
    }

    fn reply_values(
        self,
        account_id: AccountId,
    ) -> CacheResult<(MailReplySource, Option<Box<str>>)> {
        if self.recipient.is_some() {
            return Err(CacheFailure::InvalidData);
        }
        let folder_id = FolderId::try_from(self.source_folder_id.ok_or(CacheFailure::InvalidData)?)
            .map_err(|_| CacheFailure::InvalidData)?;
        let message_id =
            MessageId::try_from(self.source_message_id.ok_or(CacheFailure::InvalidData)?)
                .map_err(|_| CacheFailure::InvalidData)?;
        let source = MailReplySource::new(FolderReference::new(account_id, folder_id), message_id);
        Ok((source, self.subject.map(String::into_boxed_str)))
    }
}

fn read_pending_send(
    connection: &Connection,
    account_id: &str,
) -> CacheResult<Option<PendingMailSend>> {
    StoredPendingSend::read(connection, account_id)?
        .map(|pending| pending.into_domain(account_id))
        .transpose()
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
                received_nanoseconds, is_read, flag_status
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

fn move_cached_message(
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

fn read_cached_message(
    connection: &Connection,
    folder: &FolderReference,
    message_id: &MessageId,
) -> CacheResult<Option<(MailMessage, Option<MessageBody>)>> {
    let raw = connection
        .query_row(
            "SELECT message_id, subject, from_display_name, from_address,
                    sender_display_name, sender_address, received_seconds,
                    received_nanoseconds, is_read, flag_status, body_type, body_content
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
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
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
        flag_status: row.get(9)?,
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
    flag_status: Option<String>,
}

fn decode_message(folder: &FolderReference, raw: RawMessage) -> CacheResult<MailMessage> {
    let id = MessageId::try_from(raw.message_id).map_err(|_| CacheFailure::InvalidData)?;
    let received_at = system_time_from_parts(raw.received_seconds, raw.received_nanoseconds)?;
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
        raw.flag_status
            .as_deref()
            .map(follow_up_flag_status_from_name)
            .transpose()?,
    );
    Ok(MailMessage::new(folder.clone(), id, metadata))
}

fn read_state_from_bool(is_read: bool) -> ReadState {
    if is_read {
        ReadState::Read
    } else {
        ReadState::Unread
    }
}

fn follow_up_flag_status_from_name(name: &str) -> CacheResult<FollowUpFlagStatus> {
    match name {
        "not_flagged" => Ok(FollowUpFlagStatus::NotFlagged),
        "flagged" => Ok(FollowUpFlagStatus::Flagged),
        "complete" => Ok(FollowUpFlagStatus::Complete),
        _ => Err(CacheFailure::InvalidData),
    }
}

fn follow_up_flag_status_name(status: FollowUpFlagStatus) -> &'static str {
    match status {
        FollowUpFlagStatus::NotFlagged => "not_flagged",
        FollowUpFlagStatus::Flagged => "flagged",
        FollowUpFlagStatus::Complete => "complete",
    }
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
            "SELECT message_id, subject, from_display_name, from_address,
                    sender_display_name, sender_address, received_seconds,
                    received_nanoseconds, is_read, flag_status
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
        "INSERT INTO mail_messages (
            account_id, folder_id, message_id, subject,
            from_display_name, from_address, sender_display_name, sender_address,
            received_seconds, received_nanoseconds, is_read, flag_status,
            full_sync_generation
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT (account_id, folder_id, message_id) DO UPDATE SET
            subject = excluded.subject,
            from_display_name = excluded.from_display_name,
            from_address = excluded.from_address,
            sender_display_name = excluded.sender_display_name,
            sender_address = excluded.sender_address,
            received_seconds = excluded.received_seconds,
            received_nanoseconds = excluded.received_nanoseconds,
            is_read = excluded.is_read,
            flag_status = excluded.flag_status,
            full_sync_generation = excluded.full_sync_generation,
            body_type = NULL,
            body_content = NULL",
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
            matches!(metadata.read_state(), ReadState::Read),
            metadata
                .follow_up_flag_status()
                .map(follow_up_flag_status_name),
            full_sync_generation
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

fn begin_full_resync_transaction(
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
fn begin_checkpoint_write(connection: &mut Connection) -> CacheResult<Transaction<'_>> {
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

fn update_sync_progress(
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

fn body_type_name(body_type: MessageBodyType) -> &'static str {
    match body_type {
        MessageBodyType::Html => "html",
        MessageBodyType::Text => "text",
    }
}
