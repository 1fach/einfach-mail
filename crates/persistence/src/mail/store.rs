use std::{collections::HashSet, future::Future, time::SystemTime};

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

use crate::shared::{
    CacheLifecycleError, schema,
    store::{
        AccountScopeError, CacheFailure, CacheResult, SqliteAccountCache,
        time::{system_time_from_parts, system_time_parts},
    },
};

#[cfg(test)]
use crate::shared::store::Gate;

mod folders;
mod messages;
mod pending_send;
mod sync;
mod writes;

use self::{
    folders::{folder_exists, read_folders, replace_folder_rows, validate_folder_snapshot},
    messages::{
        decode_message, follow_up_flag_status_name, raw_message_from_row, read_cached_message,
        read_messages, read_state_from_bool,
    },
    pending_send::read_pending_send,
    sync::{
        apply_delta_change, begin_checkpoint_write, begin_full_resync_transaction, body_type_name,
        ensure_account_scope_row, read_sync_state, update_sync_progress,
    },
    writes::move_cached_message,
};

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

impl SqliteAccountCache {
    fn check_mail_account(&self, account_id: &AccountId) -> Result<(), MailCacheError> {
        self.check_account_scope(account_id)
            .map_err(|AccountScopeError::AccountMismatch| MailCacheError::AccountMismatch)
    }

    fn check_folder(&self, folder: &FolderReference) -> Result<(), MailCacheError> {
        self.check_mail_account(folder.account_id())
    }
}

impl MailCache for SqliteAccountCache {
    fn replace_folders(
        &self,
        account_id: &AccountId,
        folders: Vec<MailFolder>,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_ {
        let validation = self
            .check_mail_account(account_id)
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
            .check_mail_account(account_id)
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
        let validation = self.check_mail_account(account_id);
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
        let validation = self.check_mail_account(account_id);
        let account_id = account_id.as_str().to_owned();
        let client = self.client.clone();
        async move {
            validation?;
            #[cfg(test)]
            self.test_hooks.wait_at(Gate::PendingSendRead).await;
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
        let validation = self
            .check_mail_account(pending.account_id())
            .and_then(|()| {
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
            if !database_identity
                .matches_path(&database_path)
                .map_err(CacheLifecycleError::into_mail_cache_error)?
            {
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
        let validation = self.check_mail_account(account_id);
        let account_id = account_id.as_str().to_owned();
        let operation_id = operation_id.as_uuid().to_string();
        let client = self.client.clone();
        async move {
            validation?;
            #[cfg(test)]
            self.test_hooks.ensure_pending_send_clear_succeeds()?;
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
