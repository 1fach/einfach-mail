//! Account-scoped SQLCipher cache operations.

use std::{future::Future, sync::Arc};

#[cfg(test)]
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

use async_sqlite::Client;
use einfach_mail_application::{
    AccountCache, AccountCacheFactory, AccountCacheRemovalError, CalendarCacheError, MailCache,
    MailCacheError,
};
use einfach_mail_domain::AccountId;
use thiserror::Error;

use crate::security;

mod calendar;
mod mail;

pub(super) type CacheResult<T> = Result<T, CacheFailure>;

#[derive(Debug, Error)]
pub(super) enum CacheFailure {
    #[error("the database operation failed")]
    Unavailable,
    #[error("the database value is invalid")]
    InvalidData,
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
    #[error("another calendar event creation is already pending")]
    CreationAlreadyPending,
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
            Self::SyncStateChanged => MailCacheError::SyncStateChanged,
            Self::SendAlreadyPending => MailCacheError::SendAlreadyPending,
            Self::PendingSendMismatch => MailCacheError::PendingSendMismatch,
            Self::PendingSendNotFound => MailCacheError::PendingSendNotFound,
            Self::CreationAlreadyPending => MailCacheError::InvalidData,
        }
    }

    fn calendar_public(self) -> CalendarCacheError {
        match self {
            Self::Unavailable => CalendarCacheError::Unavailable,
            Self::InvalidData | Self::MessageNotCached => CalendarCacheError::InvalidData,
            Self::SyncStateChanged => CalendarCacheError::Unavailable,
            Self::SendAlreadyPending => CalendarCacheError::InvalidData,
            Self::PendingSendMismatch | Self::PendingSendNotFound => {
                CalendarCacheError::InvalidData
            }
            Self::CreationAlreadyPending => CalendarCacheError::CreationAlreadyPending,
        }
    }

    fn account_removal(self) -> AccountCacheRemovalError {
        match self {
            Self::Unavailable | Self::MessageNotCached | Self::SyncStateChanged => {
                AccountCacheRemovalError::Unavailable
            }
            Self::InvalidData => AccountCacheRemovalError::InvalidData,
            Self::SendAlreadyPending => AccountCacheRemovalError::PendingMailSend,
            Self::CreationAlreadyPending => AccountCacheRemovalError::PendingCalendarEventCreation,
            Self::PendingSendMismatch | Self::PendingSendNotFound => {
                AccountCacheRemovalError::Unavailable
            }
        }
    }
}

/// One account's encrypted SQLCipher database and operating-system key reference.
#[derive(Clone)]
pub struct SqliteMailCache {
    account_id: AccountId,
    client: Client,
    database_path: std::path::PathBuf,
    database_identity: security::DatabaseIdentity,
    credential_account: String,
    lifecycle_lock_path: std::path::PathBuf,
    account_lock: Arc<std::fs::File>,
    #[cfg(test)]
    fail_next_pending_send_clear: Arc<AtomicBool>,
    #[cfg(test)]
    removal_gate: Arc<Mutex<Option<TestOperationGate>>>,
    #[cfg(test)]
    pending_send_read_gate: Arc<Mutex<Option<TestOperationGate>>>,
}

#[cfg(test)]
type TestOperationGate = (smol::channel::Sender<()>, smol::channel::Receiver<()>);

/// Opens and removes account-scoped [`SqliteMailCache`] values.
#[derive(Clone, Copy, Debug, Default)]
pub struct SqliteMailCacheFactory;

impl AccountCacheFactory for SqliteMailCacheFactory {
    type Cache = SqliteMailCache;

    async fn open_for_account(&self, account_id: AccountId) -> Result<Self::Cache, MailCacheError> {
        SqliteMailCache::open_for_account(account_id).await
    }

    async fn remove_for_account(
        &self,
        account_id: &AccountId,
    ) -> Result<(), AccountCacheRemovalError> {
        SqliteMailCache::remove_for_account(account_id).await
    }
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
            database_identity: opened.database_identity,
            credential_account: opened.credential_account,
            lifecycle_lock_path: opened.lifecycle_lock_path,
            account_lock: opened.account_lock,
            #[cfg(test)]
            fail_next_pending_send_clear: Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            removal_gate: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            pending_send_read_gate: Arc::new(Mutex::new(None)),
        })
    }

    /// Removes an account's cache files and key when no cache handle is open.
    ///
    /// # Errors
    ///
    /// Returns a cache error when the cache path or credential service cannot be updated.
    pub async fn remove_for_account(
        account_id: &AccountId,
    ) -> Result<(), AccountCacheRemovalError> {
        if security::account_database_exists(account_id)? {
            let cache = Self::open_for_account(account_id.clone()).await?;
            <Self as AccountCache>::remove_account(&cache, account_id).await
        } else {
            security::remove_account_without_open_client(account_id)
                .map_err(AccountCacheRemovalError::from)
        }
    }

    #[cfg(test)]
    pub(super) async fn open_with_test_key(
        database_path: std::path::PathBuf,
        account_id: AccountId,
        key_byte: u8,
    ) -> Self {
        let opened = security::open_test_database(&database_path, key_byte)
            .await
            .expect("synthetic encrypted cache opens");
        Self {
            account_id,
            client: opened.client,
            database_path: opened.database_path,
            database_identity: opened.database_identity,
            credential_account: opened.credential_account,
            lifecycle_lock_path: opened.lifecycle_lock_path,
            account_lock: opened.account_lock,
            fail_next_pending_send_clear: Arc::new(AtomicBool::new(false)),
            removal_gate: Arc::new(Mutex::new(None)),
            pending_send_read_gate: Arc::new(Mutex::new(None)),
        }
    }

    #[cfg(test)]
    pub(super) fn hold_removal_after_marker_check_for_tests(
        &self,
    ) -> (smol::channel::Receiver<()>, smol::channel::Sender<()>) {
        let (checked_tx, checked_rx) = smol::channel::bounded(1);
        let (release_tx, release_rx) = smol::channel::bounded(1);
        *self
            .removal_gate
            .lock()
            .expect("synthetic account-removal gate mutex is healthy") =
            Some((checked_tx, release_rx));
        (checked_rx, release_tx)
    }

    #[cfg(test)]
    pub(super) fn hold_pending_send_read_for_tests(
        &self,
    ) -> (smol::channel::Receiver<()>, smol::channel::Sender<()>) {
        let (started_tx, started_rx) = smol::channel::bounded(1);
        let (release_tx, release_rx) = smol::channel::bounded(1);
        *self
            .pending_send_read_gate
            .lock()
            .expect("synthetic pending-send read gate mutex is healthy") =
            Some((started_tx, release_rx));
        (started_rx, release_tx)
    }

    #[cfg(test)]
    pub(super) fn fail_next_pending_send_clear(&self) {
        self.fail_next_pending_send_clear
            .store(true, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(super) async fn close_for_tests(&self) {
        self.client
            .close()
            .await
            .expect("synthetic encrypted cache connection closes");
    }

    #[cfg(test)]
    pub(super) async fn execute_batch_for_tests(&self, sql: &str) -> Result<(), MailCacheError> {
        let sql = sql.to_owned();
        self.client
            .conn_mut_and_then(move |connection| {
                connection.execute_batch(&sql)?;
                Ok(())
            })
            .await
            .map_err(CacheFailure::public)?;
        Ok(())
    }

    #[cfg(test)]
    pub(super) async fn set_message_move_failure_for_tests(&self, enabled: bool) {
        let statement = if enabled {
            "CREATE TRIGGER fail_message_move BEFORE DELETE ON mail_messages BEGIN SELECT RAISE(ABORT, 'synthetic move failure'); END;"
        } else {
            "DROP TRIGGER IF EXISTS fail_message_move;"
        };
        self.client
            .conn_and_then(|connection| -> Result<(), async_sqlite::Error> {
                connection.execute_batch(statement)?;
                Ok(())
            })
            .await
            .expect("synthetic message move trigger can be updated");
    }

    fn check_account(&self, account_id: &AccountId) -> Result<(), MailCacheError> {
        if &self.account_id == account_id {
            Ok(())
        } else {
            Err(MailCacheError::AccountMismatch)
        }
    }

    async fn has_unresolved_event_creation(
        &self,
        account_id: &AccountId,
    ) -> Result<bool, AccountCacheRemovalError> {
        self.check_account(account_id)
            .map_err(AccountCacheRemovalError::from)?;
        let account_id = account_id.as_str().to_owned();
        self.client
            .conn_and_then(move |connection| -> CacheResult<bool> {
                connection
                    .query_row(
                        "SELECT EXISTS (
                            SELECT 1 FROM calendar_pending_event_creation
                            WHERE account_id = ?1 AND resolution = 'unresolved'
                        )",
                        [account_id],
                        |row| row.get::<_, bool>(0),
                    )
                    .map_err(CacheFailure::from)
            })
            .await
            .map_err(CacheFailure::account_removal)
    }

    async fn remove_account_under_lock(
        &self,
        account_id: &AccountId,
        removal_lock: security::AccountRemovalLock,
    ) -> Result<(), AccountCacheRemovalError> {
        match self.remove_account_contents(account_id).await {
            Ok(()) => {
                removal_lock.retain_exclusive();
                Ok(())
            }
            Err(error) => {
                removal_lock.restore_shared()?;
                Err(error)
            }
        }
    }

    async fn remove_account_contents(
        &self,
        account_id: &AccountId,
    ) -> Result<(), AccountCacheRemovalError> {
        if !self.database_identity.matches_path(&self.database_path)? {
            return Err(AccountCacheRemovalError::Unavailable);
        }
        if self.load_pending_send(account_id).await?.is_some() {
            return Err(AccountCacheRemovalError::PendingMailSend);
        }
        if self.has_unresolved_event_creation(account_id).await? {
            return Err(AccountCacheRemovalError::PendingCalendarEventCreation);
        }
        #[cfg(test)]
        self.wait_for_removal_test_gate().await;
        security::delete_database_files(&self.client, &self.database_path).await?;
        security::remove_credential(&self.credential_account)
            .map_err(AccountCacheRemovalError::from)
    }

    #[cfg(test)]
    async fn wait_for_removal_test_gate(&self) {
        let gate = self
            .removal_gate
            .lock()
            .expect("synthetic account-removal gate mutex is healthy")
            .take();
        if let Some((checked, release)) = gate {
            let _ = checked.send(()).await;
            let _ = release.recv().await;
        }
    }
}

impl AccountCache for SqliteMailCache {
    fn remove_account(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = Result<(), AccountCacheRemovalError>> + Send + '_ {
        let validation = self.check_account(account_id);
        let account_id = account_id.clone();
        let lifecycle_lock_path = self.lifecycle_lock_path.clone();
        let account_lock = Arc::clone(&self.account_lock);
        async move {
            validation.map_err(AccountCacheRemovalError::from)?;
            let removal_lock =
                security::AccountRemovalLock::acquire(&lifecycle_lock_path, account_lock)
                    .map_err(AccountCacheRemovalError::from)?;
            self.remove_account_under_lock(&account_id, removal_lock)
                .await
        }
    }
}

#[cfg(test)]
mod tests;
