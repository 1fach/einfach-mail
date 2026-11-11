use std::{
    future::{Future, ready},
    sync::Arc,
};

use einfach_mail_application::{
    AccountCacheFactory, AccountCacheOpenError, AccountCacheRemovalError, AccountSession,
    AccountSessionFactory, AccountSessionWorkflowFactory,
};
use einfach_mail_domain::AccountId;
use tempfile::TempDir;

use crate::SqliteAccountCache;

use super::provider::FixtureProvider;

pub(crate) const DATABASE_KEY_BYTE: u8 = 0x62;

pub(crate) struct FixtureCacheFactory {
    pub(crate) database_directory: std::path::PathBuf,
}

pub(crate) struct RetainedCacheFactory {
    pub(crate) account_id: AccountId,
    pub(crate) cache: SqliteAccountCache,
}

impl AccountCacheFactory for FixtureCacheFactory {
    type Cache = SqliteAccountCache;

    fn open_for_account(
        &self,
        account_id: AccountId,
    ) -> impl Future<Output = Result<Self::Cache, AccountCacheOpenError>> + Send + '_ {
        let database_path = self.database_directory.join("mail.sqlite3");
        async move {
            Ok(
                SqliteAccountCache::open_with_test_key(
                    database_path,
                    account_id,
                    DATABASE_KEY_BYTE,
                )
                .await,
            )
        }
    }

    fn remove_for_account<'a>(
        &'a self,
        _: &'a AccountId,
    ) -> impl Future<Output = Result<(), AccountCacheRemovalError>> + Send + 'a {
        ready(Err(AccountCacheRemovalError::Unavailable))
    }
}

impl AccountCacheFactory for RetainedCacheFactory {
    type Cache = SqliteAccountCache;

    fn open_for_account(
        &self,
        account_id: AccountId,
    ) -> impl Future<Output = Result<Self::Cache, AccountCacheOpenError>> + Send + '_ {
        ready(if account_id == self.account_id {
            Ok(self.cache.clone())
        } else {
            Err(AccountCacheOpenError::AccountMismatch)
        })
    }

    fn remove_for_account<'a>(
        &'a self,
        _: &'a AccountId,
    ) -> impl Future<Output = Result<(), AccountCacheRemovalError>> + Send + 'a {
        ready(Err(AccountCacheRemovalError::Unavailable))
    }
}

pub(crate) fn account_id() -> AccountId {
    AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid")
}

pub(crate) fn assert_removal_completed(
    result: Result<(), AccountCacheRemovalError>,
    database_path: &std::path::Path,
) {
    assert_ne!(result, Err(AccountCacheRemovalError::OperationInProgress));
    assert!(!database_path.exists());
}

pub(crate) async fn open_cache(directory: &TempDir, account_id: AccountId) -> SqliteAccountCache {
    SqliteAccountCache::open_with_test_key(
        directory.path().join("mail.sqlite3"),
        account_id,
        DATABASE_KEY_BYTE,
    )
    .await
}

pub(crate) async fn open_session_over_retained_cache(
    provider: FixtureProvider,
    account_id: &AccountId,
    cache: &SqliteAccountCache,
) -> Arc<dyn AccountSession> {
    let factory = AccountSessionWorkflowFactory::new(
        provider,
        RetainedCacheFactory {
            account_id: account_id.clone(),
            cache: cache.clone(),
        },
    );
    factory
        .open_session(account_id.clone())
        .await
        .expect("synthetic account session opens over its retained cache")
}
