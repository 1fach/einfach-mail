//! Linux cache paths, file locking, and SQLCipher key access.

use std::{
    fs::{self, File, OpenOptions, Permissions},
    io,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};

use async_sqlite::{Client, ClientBuilder};
use einfach_mail_domain::AccountId;
use keyring::{Entry, Error as KeyringError};
use sha2::{Digest, Sha256};
use xdg::BaseDirectories;
use zeroize::Zeroizing;

use crate::shared::{CacheLifecycleError, schema, store::CacheFailure};

const KEY_BYTES: usize = 32;
const CREDENTIAL_SERVICE: &str = "com.1fach.einfach-mail";

pub(crate) struct OpenDatabase {
    pub(crate) client: Client,
    pub(crate) database_path: PathBuf,
    pub(crate) database_identity: DatabaseIdentity,
    pub(crate) lifecycle_lock_path: PathBuf,
    pub(crate) credential_account: String,
    pub(crate) account_lock: Arc<File>,
}

struct CachePaths {
    database: PathBuf,
    account_lock: PathBuf,
    lifecycle_lock: PathBuf,
    credential_account: String,
}

struct CacheOpenLocks {
    account_lock: Arc<File>,
    _initialization_lock: File,
}

/// Holds the lifecycle gate while an open cache attempts account removal.
pub(crate) struct AccountRemovalLock {
    account_lock: Arc<File>,
    lifecycle_lock: Option<File>,
    restore_shared_on_drop: bool,
}

impl AccountRemovalLock {
    /// Acquires the lifecycle gate and attempts to make the open cache exclusive.
    ///
    /// # Errors
    ///
    /// Returns [`CacheLifecycleError::InUse`] when another cache handle or lifecycle operation
    /// prevents removal.
    /// Returns [`CacheLifecycleError::Unavailable`] when lock state cannot be restored.
    pub(crate) fn acquire(
        lifecycle_lock_path: &Path,
        account_lock: Arc<File>,
    ) -> Result<Self, CacheLifecycleError> {
        let lifecycle_lock = lock_account_directory_exclusive(lifecycle_lock_path)?;
        let mut removal_lock = Self {
            account_lock,
            lifecycle_lock: Some(lifecycle_lock),
            restore_shared_on_drop: true,
        };
        if let Err(error) = removal_lock.account_lock.try_lock() {
            restore_shared_account_lock(&removal_lock.account_lock)?;
            removal_lock.restore_shared_on_drop = false;
            return Err(lock_error(error));
        }
        Ok(removal_lock)
    }

    /// Restores shared ownership before releasing the lifecycle gate.
    ///
    /// # Errors
    ///
    /// Returns [`CacheLifecycleError::InUse`] when the account lock cannot be reacquired.
    /// Returns [`CacheLifecycleError::Unavailable`] when shared ownership cannot be restored.
    pub(crate) fn restore_shared(mut self) -> Result<(), CacheLifecycleError> {
        restore_shared_account_lock(&self.account_lock)?;
        self.restore_shared_on_drop = false;
        Ok(())
    }

    /// Keeps exclusive ownership after successful removal until the cache handle drops.
    pub(crate) fn retain_exclusive(mut self) {
        self.restore_shared_on_drop = false;
    }
}

impl Drop for AccountRemovalLock {
    fn drop(&mut self) {
        if self.restore_shared_on_drop {
            let restore_result = restore_shared_account_lock(&self.account_lock);
            if let Some(lifecycle_lock) =
                take_lifecycle_lock_if_restore_failed(restore_result, &mut self.lifecycle_lock)
            {
                // Block new opens and removals until process exit if shared ownership is lost.
                std::mem::forget(lifecycle_lock);
            }
        }
    }
}

fn take_lifecycle_lock_if_restore_failed(
    restore_result: Result<(), CacheLifecycleError>,
    lifecycle_lock: &mut Option<File>,
) -> Option<File> {
    restore_result.err()?;
    lifecycle_lock.take()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DatabaseIdentity {
    device: u64,
    inode: u64,
}

impl DatabaseIdentity {
    pub(crate) fn read(path: &Path) -> Result<Self, CacheLifecycleError> {
        let metadata = fs::metadata(path).map_err(|_| CacheLifecycleError::Unavailable)?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    pub(crate) fn matches_path(self, path: &Path) -> Result<bool, CacheLifecycleError> {
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(_) => return Err(CacheLifecycleError::Unavailable),
        };
        Ok(self.device == metadata.dev() && self.inode == metadata.ino())
    }
}

struct DatabaseKey(Zeroizing<[u8; KEY_BYTES]>);

enum KeyLookup {
    Existing(DatabaseKey),
    Create,
    Missing,
}

impl DatabaseKey {
    fn generate() -> Result<Self, CacheLifecycleError> {
        let mut bytes = Zeroizing::new([0_u8; KEY_BYTES]);
        getrandom::fill(bytes.as_mut()).map_err(|_| CacheLifecycleError::Unavailable)?;
        Ok(Self(bytes))
    }

    fn from_secret(secret: Vec<u8>) -> Result<Self, CacheLifecycleError> {
        let secret = Zeroizing::new(secret);
        if secret.len() != KEY_BYTES {
            return Err(CacheLifecycleError::MissingEncryptionKey);
        }
        let mut bytes = Zeroizing::new([0_u8; KEY_BYTES]);
        bytes.as_mut().copy_from_slice(secret.as_slice());
        Ok(Self(bytes))
    }

    fn raw_key_statement(&self) -> Zeroizing<String> {
        let encoded = Zeroizing::new(hex::encode(self.0.as_slice()));
        let mut statement = Zeroizing::new(String::with_capacity(82));
        statement.push_str("PRAGMA key = \"x'");
        statement.push_str(&encoded);
        statement.push_str("'\"");
        statement
    }
}

pub(crate) async fn open_database(
    account_id: &AccountId,
) -> Result<OpenDatabase, CacheLifecycleError> {
    let paths = cache_paths(account_id)?;
    let CacheOpenLocks {
        account_lock,
        _initialization_lock,
    } = acquire_cache_open_locks(&paths)?;
    open_locked_database(paths, account_lock).await
}

fn acquire_cache_open_locks(paths: &CachePaths) -> Result<CacheOpenLocks, CacheLifecycleError> {
    let lifecycle_lock = lock_account_directory_shared(&paths.lifecycle_lock)?;
    let account_lock = Arc::new(lock_account_directory_shared(&paths.account_lock)?);
    drop(lifecycle_lock);
    let initialization_lock_path = with_suffix(&paths.account_lock, ".init");
    let initialization_lock = lock_account_directory_exclusive(&initialization_lock_path)?;
    Ok(CacheOpenLocks {
        account_lock,
        _initialization_lock: initialization_lock,
    })
}

async fn open_locked_database(
    paths: CachePaths,
    account_lock: Arc<File>,
) -> Result<OpenDatabase, CacheLifecycleError> {
    let credential = credential_entry(&paths.credential_account)?;
    let key = load_or_create_key(&credential, encrypted_data_exists(&paths.database)?)?;
    open_database_with_key(paths, key, account_lock).await
}

async fn open_database_with_key(
    paths: CachePaths,
    key: DatabaseKey,
    account_lock: Arc<File>,
) -> Result<OpenDatabase, CacheLifecycleError> {
    secure_database_file(&paths.database)?;
    let client = open_encrypted_client(&paths.database, &key).await?;
    schema::apply(&client)
        .await
        .map_err(CacheFailure::into_lifecycle_error)?;
    let database_identity = DatabaseIdentity::read(&paths.database)?;
    Ok(OpenDatabase {
        client,
        database_path: paths.database,
        database_identity,
        lifecycle_lock_path: paths.lifecycle_lock,
        credential_account: paths.credential_account,
        account_lock,
    })
}

fn load_or_create_key(
    credential: &Entry,
    database_exists: bool,
) -> Result<DatabaseKey, CacheLifecycleError> {
    match classify_key_lookup(credential.get_secret(), database_exists)? {
        KeyLookup::Existing(key) => Ok(key),
        KeyLookup::Create => create_and_store_key(credential),
        KeyLookup::Missing => Err(CacheLifecycleError::MissingEncryptionKey),
    }
}

fn classify_key_lookup(
    secret_result: keyring::Result<Vec<u8>>,
    database_exists: bool,
) -> Result<KeyLookup, CacheLifecycleError> {
    match secret_result {
        Ok(secret) => DatabaseKey::from_secret(secret).map(KeyLookup::Existing),
        Err(KeyringError::NoEntry) if database_exists => Ok(KeyLookup::Missing),
        Err(KeyringError::NoEntry) => Ok(KeyLookup::Create),
        Err(_) => Err(CacheLifecycleError::Unavailable),
    }
}

fn create_and_store_key(credential: &Entry) -> Result<DatabaseKey, CacheLifecycleError> {
    let key = DatabaseKey::generate()?;
    credential
        .set_secret(key.0.as_slice())
        .map_err(|_| CacheLifecycleError::Unavailable)?;
    Ok(key)
}

async fn open_encrypted_client(
    database_path: &Path,
    key: &DatabaseKey,
) -> Result<Client, CacheLifecycleError> {
    let client = ClientBuilder::new()
        .path(database_path)
        .open()
        .await
        .map_err(|_| CacheLifecycleError::Unavailable)?;
    let key_statement = key.raw_key_statement();
    client
        .conn_mut(move |connection| connection.execute_batch(&key_statement))
        .await
        .map_err(|_| CacheLifecycleError::Unavailable)?;
    client
        .conn_mut(|connection| connection.execute_batch("PRAGMA foreign_keys = ON"))
        .await
        .map_err(|_| CacheLifecycleError::Unavailable)?;
    Ok(client)
}

#[cfg(test)]
pub(crate) async fn open_test_database(
    database_path: &Path,
    key_byte: u8,
) -> Result<OpenDatabase, CacheLifecycleError> {
    let account_directory = database_path
        .parent()
        .ok_or(CacheLifecycleError::Unavailable)?;
    let paths = CachePaths {
        database: database_path.to_owned(),
        account_lock: account_directory.join("mail-cache.lock"),
        lifecycle_lock: account_directory.join("mail-cache.lifecycle.lock"),
        credential_account: "synthetic-cache-key".to_owned(),
    };
    let CacheOpenLocks {
        account_lock,
        _initialization_lock,
    } = acquire_cache_open_locks(&paths)?;
    let key = DatabaseKey(Zeroizing::new([key_byte; KEY_BYTES]));
    open_database_with_key(paths, key, account_lock).await
}

fn cache_paths(account_id: &AccountId) -> Result<CachePaths, CacheLifecycleError> {
    let project = BaseDirectories::with_prefix("einfach-mail");
    let account_hash = hex::encode(Sha256::digest(account_id.as_str().as_bytes()));
    let account_directory = project
        .get_data_file(PathBuf::from("mail-cache").join(&account_hash))
        .ok_or(CacheLifecycleError::Unavailable)?;
    Ok(CachePaths {
        database: account_directory.join("mail.sqlite3"),
        account_lock: account_directory.join("mail-cache.lock"),
        lifecycle_lock: account_directory.join("mail-cache.lifecycle.lock"),
        credential_account: format!("mail-cache-key-{account_hash}"),
    })
}

fn create_private_directory(path: &Path) -> Result<(), CacheLifecycleError> {
    fs::create_dir_all(path).map_err(|_| CacheLifecycleError::Unavailable)?;
    fs::set_permissions(path, Permissions::from_mode(0o700))
        .map_err(|_| CacheLifecycleError::Unavailable)
}

fn open_lock_file(path: &Path) -> Result<File, CacheLifecycleError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .map_err(|_| CacheLifecycleError::Unavailable)?;
    file.set_permissions(Permissions::from_mode(0o600))
        .map_err(|_| CacheLifecycleError::Unavailable)?;
    Ok(file)
}

fn secure_database_file(path: &Path) -> Result<(), CacheLifecycleError> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .map_err(|_| CacheLifecycleError::Unavailable)?
        .set_permissions(Permissions::from_mode(0o600))
        .map_err(|_| CacheLifecycleError::Unavailable)
}

fn encrypted_data_exists(database_path: &Path) -> Result<bool, CacheLifecycleError> {
    if path_exists(database_path)? {
        return Ok(true);
    }
    if path_exists(&with_suffix(database_path, "-wal"))? {
        return Ok(true);
    }
    path_exists(&with_suffix(database_path, "-shm"))
}

fn path_exists(path: &Path) -> Result<bool, CacheLifecycleError> {
    path.try_exists()
        .map_err(|_| CacheLifecycleError::Unavailable)
}

pub(crate) fn account_database_exists(account_id: &AccountId) -> Result<bool, CacheLifecycleError> {
    encrypted_data_exists(&cache_paths(account_id)?.database)
}

fn credential_entry(account: &str) -> Result<Entry, CacheLifecycleError> {
    Entry::new(CREDENTIAL_SERVICE, account).map_err(|_| CacheLifecycleError::Unavailable)
}

pub(crate) fn remove_credential(account: &str) -> Result<(), CacheLifecycleError> {
    match credential_entry(account)?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(_) => Err(CacheLifecycleError::Unavailable),
    }
}

pub(crate) async fn delete_database_files(
    client: &Client,
    database_path: &Path,
) -> Result<(), CacheLifecycleError> {
    client
        .close()
        .await
        .map_err(|_| CacheLifecycleError::Unavailable)?;
    remove_database_files(database_path)
}

pub(crate) fn remove_account_without_open_client(
    account_id: &AccountId,
) -> Result<(), CacheLifecycleError> {
    let paths = cache_paths(account_id)?;
    remove_unopened_account_cache(
        &paths.database,
        &paths.account_lock,
        &paths.lifecycle_lock,
        || remove_credential(&paths.credential_account),
    )
}

fn remove_unopened_account_cache(
    database_path: &Path,
    account_lock_path: &Path,
    lifecycle_lock_path: &Path,
    remove_credential: impl FnOnce() -> Result<(), CacheLifecycleError>,
) -> Result<(), CacheLifecycleError> {
    let _lifecycle_lock = lock_account_directory_exclusive(lifecycle_lock_path)?;
    let _account_lock = lock_account_directory_exclusive(account_lock_path)?;
    if encrypted_data_exists(database_path)? {
        remove_database_files(database_path)?;
    }
    remove_credential()
}

fn restore_shared_account_lock(file: &File) -> Result<(), CacheLifecycleError> {
    file.unlock()
        .map_err(|_| CacheLifecycleError::Unavailable)?;
    file.try_lock_shared().map_err(lock_error)
}

/// Allows multiple cache sessions to share the account lock while excluding removal.
///
/// # Errors
///
/// Returns [`CacheLifecycleError::InUse`] when an account removal owns the lock.
/// Returns [`CacheLifecycleError::Unavailable`] when the lock file cannot be opened or locked.
fn lock_account_directory_shared(path: &Path) -> Result<File, CacheLifecycleError> {
    let file = open_account_lock_file(path)?;
    file.try_lock_shared().map_err(lock_error)?;
    Ok(file)
}

fn lock_account_directory_exclusive(path: &Path) -> Result<File, CacheLifecycleError> {
    let file = open_account_lock_file(path)?;
    file.try_lock().map_err(lock_error)?;
    Ok(file)
}

fn open_account_lock_file(path: &Path) -> Result<File, CacheLifecycleError> {
    let account_directory = path.parent().ok_or(CacheLifecycleError::Unavailable)?;
    create_private_directory(account_directory)?;
    open_lock_file(path)
}

fn lock_error(error: std::fs::TryLockError) -> CacheLifecycleError {
    match error {
        std::fs::TryLockError::WouldBlock => CacheLifecycleError::InUse,
        std::fs::TryLockError::Error(_) => CacheLifecycleError::Unavailable,
    }
}

fn remove_database_files(database_path: &Path) -> Result<(), CacheLifecycleError> {
    // Remove sidecars first so a failure leaves the database available for retry.
    remove_file_if_present(&with_suffix(database_path, "-wal"))?;
    remove_file_if_present(&with_suffix(database_path, "-shm"))?;
    remove_file_if_present(database_path)
}

fn remove_file_if_present(path: &Path) -> Result<(), CacheLifecycleError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(CacheLifecycleError::Unavailable),
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(suffix);
    PathBuf::from(value)
}

#[cfg(test)]
mod tests;
