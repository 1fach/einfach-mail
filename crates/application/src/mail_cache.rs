//! Local mail storage contracts used by synchronization and cached browsing.

use std::{future::Future, time::SystemTime};

use einfach_mail_domain::{
    AccountId, FolderReference, MailFolder, MailMessage, MessageBody, MessageDeltaLink,
};
use thiserror::Error;

use crate::MessageDeltaRequest;

/// A user-safe category for a local mail cache failure.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum MailCacheError {
    /// The encrypted cache or its credential service is unavailable.
    #[error("the local mail cache is unavailable")]
    Unavailable,
    /// The cache exists but its encryption key is missing or malformed.
    #[error("the local mail cache encryption key is unavailable")]
    MissingEncryptionKey,
    /// Stored values do not satisfy the current mail domain contract.
    #[error("the local mail cache contains invalid data")]
    InvalidData,
    /// The requested account does not own the cache or folder reference.
    #[error("the local mail cache belongs to another account")]
    AccountMismatch,
    /// The requested message is not present in the cached folder.
    #[error("the message is not in the local mail cache")]
    MessageNotCached,
}

/// A folder's account-wide cutoff and message delta checkpoints.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MailSyncState {
    cutoff: SystemTime,
    delta_link: Option<MessageDeltaLink>,
    next_link: Option<MessageDeltaLink>,
    last_synchronized_at: Option<SystemTime>,
}

impl MailSyncState {
    /// Creates state for a folder whose initial delta query has not started.
    #[must_use]
    pub fn initial(cutoff: SystemTime) -> Self {
        Self {
            cutoff,
            delta_link: None,
            next_link: None,
            last_synchronized_at: None,
        }
    }

    /// Restores a checkpoint after the persistence boundary validates its stored timestamps.
    ///
    /// # Errors
    ///
    /// Returns [`MailCacheError::InvalidData`] when a completion time exists
    /// without a completed delta link.
    pub fn from_checkpoint(
        cutoff: SystemTime,
        delta_link: Option<MessageDeltaLink>,
        next_link: Option<MessageDeltaLink>,
        last_synchronized_at: Option<SystemTime>,
    ) -> Result<Self, MailCacheError> {
        if last_synchronized_at.is_some() && delta_link.is_none() {
            return Err(MailCacheError::InvalidData);
        }
        Ok(Self {
            cutoff,
            delta_link,
            next_link,
            last_synchronized_at,
        })
    }

    /// Returns the fixed cutoff shared by this account's mail folders.
    #[must_use]
    pub fn cutoff(&self) -> SystemTime {
        self.cutoff
    }

    /// Returns the completed round's opaque delta link, if one exists.
    #[must_use]
    pub fn delta_link(&self) -> Option<&MessageDeltaLink> {
        self.delta_link.as_ref()
    }

    /// Returns the in-progress page link, if a round was interrupted.
    #[must_use]
    pub fn next_link(&self) -> Option<&MessageDeltaLink> {
        self.next_link.as_ref()
    }

    /// Returns the time when the last complete round committed.
    #[must_use]
    pub fn last_synchronized_at(&self) -> Option<SystemTime> {
        self.last_synchronized_at
    }

    /// Selects the next request while preferring an interrupted round checkpoint.
    #[must_use]
    pub fn request(&self) -> MessageDeltaRequest {
        self.next_link
            .as_ref()
            .or(self.delta_link.as_ref())
            .map_or(MessageDeltaRequest::Initial, |link| {
                MessageDeltaRequest::Continue(link.clone())
            })
    }

    /// Returns whether a complete delta link has been committed without a pending page.
    #[must_use]
    pub fn is_synchronized(&self) -> bool {
        self.delta_link.is_some() && self.next_link.is_none()
    }
}

/// Reads and atomically updates the encrypted local mail cache.
pub trait MailCache: Send + Sync {
    /// Replaces one account's visible folder tree and preserves matching cached records.
    ///
    /// # Errors
    ///
    /// Returns [`MailCacheError::AccountMismatch`] for a different cache account,
    /// [`MailCacheError::InvalidData`] for inconsistent folder identities, or
    /// [`MailCacheError::Unavailable`] when the transaction fails.
    fn replace_folders(
        &self,
        account_id: &AccountId,
        folders: Vec<MailFolder>,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_;

    /// Stores the account's initial cutoff once, including when it has no visible folders.
    ///
    /// # Errors
    ///
    /// Returns [`MailCacheError::AccountMismatch`] for another account,
    /// [`MailCacheError::InvalidData`] when the stored query scope is unsupported,
    /// or [`MailCacheError::Unavailable`] when the transaction fails.
    fn ensure_account_sync_scope(
        &self,
        account_id: &AccountId,
        initial_cutoff: SystemTime,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_;

    /// Returns the fixed cutoff and cursor state, creating them only for a new folder scope.
    ///
    /// # Errors
    ///
    /// Returns [`MailCacheError::AccountMismatch`] for a different cache account,
    /// [`MailCacheError::InvalidData`] when the folder is not cached, or
    /// [`MailCacheError::Unavailable`] when the transaction fails.
    fn ensure_sync_state(
        &self,
        folder: &FolderReference,
        initial_cutoff: SystemTime,
    ) -> impl Future<Output = Result<MailSyncState, MailCacheError>> + Send + '_;

    /// Returns a folder's existing sync state without creating a new cutoff.
    ///
    /// # Errors
    ///
    /// Returns [`MailCacheError::AccountMismatch`] for another account,
    /// [`MailCacheError::InvalidData`] when persisted state is malformed, or
    /// [`MailCacheError::Unavailable`] when the cache cannot be read.
    fn load_sync_state(
        &self,
        folder: &FolderReference,
    ) -> impl Future<Output = Result<Option<MailSyncState>, MailCacheError>> + Send + '_;

    /// Applies one page and advances its checkpoint in the same transaction.
    ///
    /// The store reads the fixed cutoff from the scope. A `Next` link updates
    /// only the in-progress checkpoint; a `Complete` link replaces the durable
    /// delta link and records the completion time.
    ///
    /// # Errors
    ///
    /// Returns [`MailCacheError::AccountMismatch`] for a different cache account,
    /// [`MailCacheError::InvalidData`] for malformed cached values, or
    /// [`MailCacheError::Unavailable`] when the transaction fails.
    fn commit_delta_page(
        &self,
        folder: &FolderReference,
        page: einfach_mail_domain::MessageDeltaPage,
        completed_at: SystemTime,
    ) -> impl Future<Output = Result<MailSyncState, MailCacheError>> + Send + '_;

    /// Returns all visible folders saved for `account_id`.
    ///
    /// # Errors
    ///
    /// Returns [`MailCacheError::AccountMismatch`] for a different cache account,
    /// [`MailCacheError::InvalidData`] for malformed stored folders, or
    /// [`MailCacheError::Unavailable`] when the cache cannot be read.
    fn list_folders(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<Vec<MailFolder>, MailCacheError>> + Send + '_;

    /// Returns cached message metadata in newest-first order.
    ///
    /// # Errors
    ///
    /// Returns [`MailCacheError::AccountMismatch`] for another account,
    /// [`MailCacheError::InvalidData`] for malformed stored messages, or
    /// [`MailCacheError::Unavailable`] when the cache cannot be read.
    fn list_messages(
        &self,
        folder: &FolderReference,
    ) -> impl Future<Output = Result<Vec<MailMessage>, MailCacheError>> + Send + '_;

    /// Returns cached metadata and body for one message.
    ///
    /// # Errors
    ///
    /// Returns [`MailCacheError::AccountMismatch`] for another account,
    /// [`MailCacheError::InvalidData`] for malformed stored messages, or
    /// [`MailCacheError::Unavailable`] when the cache cannot be read.
    fn read_message(
        &self,
        folder: &FolderReference,
        message_id: &einfach_mail_domain::MessageId,
    ) -> impl Future<Output = Result<Option<(MailMessage, Option<MessageBody>)>, MailCacheError>>
    + Send
    + '_;

    /// Stores an opened raw body only when its message remains in the folder cache.
    ///
    /// # Errors
    ///
    /// Returns [`MailCacheError::AccountMismatch`] for another account,
    /// [`MailCacheError::MessageNotCached`] when the message is absent,
    /// [`MailCacheError::InvalidData`] for an unsupported body type, or
    /// [`MailCacheError::Unavailable`] when the write fails.
    fn store_message_body(
        &self,
        folder: &FolderReference,
        message_id: &einfach_mail_domain::MessageId,
        body: MessageBody,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_;

    /// Removes an account's encrypted data and its local key.
    ///
    /// # Errors
    ///
    /// Returns [`MailCacheError::AccountMismatch`] for another account or
    /// [`MailCacheError::Unavailable`] when the credential or files cannot be removed.
    fn remove_account(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + '_;
}

/// Opens and removes account-scoped mail caches.
pub trait MailCacheFactory: Send + Sync {
    /// The cache implementation opened for each account.
    type Cache: MailCache + 'static;

    /// Opens or creates the cache owned by `account_id`.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the account cache or its credential is unavailable.
    fn open_for_account(
        &self,
        account_id: AccountId,
    ) -> impl Future<Output = Result<Self::Cache, MailCacheError>> + Send + '_;

    /// Removes the cache and encryption key owned by `account_id`.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the account files or credential cannot be removed.
    fn remove_for_account<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> impl Future<Output = Result<(), MailCacheError>> + Send + 'a;
}
