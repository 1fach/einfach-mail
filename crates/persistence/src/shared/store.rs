//! Account-scoped SQLCipher cache operations.

use std::{future::Future, sync::Arc};

use async_sqlite::Client;
use einfach_mail_application::{
    AccountCache, AccountCacheFactory, AccountCacheOpenError, AccountCacheRemovalError,
    CalendarCacheError, MailCache, MailCacheError,
};
use einfach_mail_domain::AccountId;
use thiserror::Error;

use crate::platform::linux::security;

#[cfg(test)]
mod test_hooks;

#[cfg(test)]
pub(crate) use self::test_hooks::{Gate, TestHooks};

pub(crate) mod time;

pub(crate) type CacheResult<T> = Result<T, CacheFailure>;

#[derive(Debug, Error)]
pub(crate) enum CacheFailure {
    #[error("the database operation failed")]
    Unavailable,
    #[error("the database value is invalid")]
    InvalidData,
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
    pub(crate) fn mail_public(self) -> MailCacheError {
        match self {
            Self::Unavailable => MailCacheError::Unavailable,
            Self::InvalidData => MailCacheError::InvalidData,
        }
    }

    pub(crate) fn calendar_public(self) -> CalendarCacheError {
        match self {
            Self::Unavailable => CalendarCacheError::Unavailable,
            Self::InvalidData => CalendarCacheError::InvalidData,
        }
    }

    fn account_removal(self) -> AccountCacheRemovalError {
        match self {
            Self::Unavailable => AccountCacheRemovalError::Unavailable,
            Self::InvalidData => AccountCacheRemovalError::InvalidData,
        }
    }
}

/// One account's encrypted SQLCipher database and operating-system key reference.
#[derive(Clone)]
pub struct SqliteAccountCache {
    pub(crate) account_id: AccountId,
    pub(crate) client: Client,
    pub(crate) database_path: std::path::PathBuf,
    pub(crate) database_identity: security::DatabaseIdentity,
    credential_account: String,
    lifecycle_lock_path: std::path::PathBuf,
    account_lock: Arc<std::fs::File>,
    #[cfg(test)]
    pub(crate) test_hooks: Arc<TestHooks>,
}

/// Opens and removes account-scoped [`SqliteAccountCache`] values.
#[derive(Clone, Copy, Debug, Default)]
pub struct SqliteAccountCacheFactory;

impl AccountCacheFactory for SqliteAccountCacheFactory {
    type Cache = SqliteAccountCache;

    async fn open_for_account(
        &self,
        account_id: AccountId,
    ) -> Result<Self::Cache, AccountCacheOpenError> {
        SqliteAccountCache::open_for_account(account_id).await
    }

    async fn remove_for_account(
        &self,
        account_id: &AccountId,
    ) -> Result<(), AccountCacheRemovalError> {
        SqliteAccountCache::remove_for_account(account_id).await
    }
}

impl SqliteAccountCache {
    /// Opens or creates the account's encrypted cache under the Linux data directory.
    ///
    /// A new 32-byte key is generated only when neither the database nor its
    /// sidecar files exist and Secret Service has no saved key.
    ///
    /// # Errors
    ///
    /// Returns [`AccountCacheOpenError::Unavailable`] when paths, Secret Service, SQLCipher, or
    /// migrations cannot be accessed. Returns [`AccountCacheOpenError::InUse`] when another
    /// lifecycle operation holds the account lock. Returns
    /// [`AccountCacheOpenError::MissingEncryptionKey`] when an existing cache has no usable key.
    /// Returns [`AccountCacheOpenError::InvalidData`] when stored values are invalid.
    pub async fn open_for_account(account_id: AccountId) -> Result<Self, AccountCacheOpenError> {
        let opened = security::open_database(&account_id)
            .await
            .map_err(AccountCacheOpenError::from)?;
        Ok(Self {
            account_id,
            client: opened.client,
            database_path: opened.database_path,
            database_identity: opened.database_identity,
            credential_account: opened.credential_account,
            lifecycle_lock_path: opened.lifecycle_lock_path,
            account_lock: opened.account_lock,
            #[cfg(test)]
            test_hooks: Arc::new(TestHooks::default()),
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
    pub(crate) async fn open_with_test_key(
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
            test_hooks: Arc::new(TestHooks::default()),
        }
    }

    #[cfg(test)]
    pub(crate) fn hold_removal_after_marker_check_for_tests(
        &self,
    ) -> (smol::channel::Receiver<()>, smol::channel::Sender<()>) {
        self.test_hooks.hold_at(Gate::RemovalAfterMarkerCheck)
    }

    #[cfg(test)]
    pub(crate) fn hold_pending_send_read_for_tests(
        &self,
    ) -> (smol::channel::Receiver<()>, smol::channel::Sender<()>) {
        self.test_hooks.hold_at(Gate::PendingSendRead)
    }

    #[cfg(test)]
    pub(crate) fn fail_next_pending_send_clear(&self) {
        self.test_hooks.fail_next_pending_send_clear();
    }

    #[cfg(test)]
    pub(crate) async fn close_for_tests(&self) {
        self.client
            .close()
            .await
            .expect("synthetic encrypted cache connection closes");
    }

    #[cfg(test)]
    pub(crate) async fn execute_batch_for_tests(&self, sql: &str) -> Result<(), MailCacheError> {
        let sql = sql.to_owned();
        self.client
            .conn_mut_and_then(move |connection| {
                connection.execute_batch(&sql)?;
                Ok(())
            })
            .await
            .map_err(CacheFailure::mail_public)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn set_message_move_failure_for_tests(&self, enabled: bool) {
        TestHooks::set_message_move_failure(&self.client, enabled).await;
    }

    pub(crate) fn check_account(&self, account_id: &AccountId) -> Result<(), MailCacheError> {
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
        self.test_hooks.wait_at(Gate::RemovalAfterMarkerCheck).await;
        security::delete_database_files(&self.client, &self.database_path).await?;
        security::remove_credential(&self.credential_account)
            .map_err(AccountCacheRemovalError::from)
    }
}

impl AccountCache for SqliteAccountCache {
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
