//! Linux cache paths, file locking, and SQLCipher key access.

use std::{
    fs::{self, File, OpenOptions, Permissions},
    io,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};

use async_sqlite::{Client, ClientBuilder};
use einfach_mail_application::MailCacheError;
use einfach_mail_domain::AccountId;
use keyring::{Entry, Error as KeyringError};
use sha2::{Digest, Sha256};
use xdg::BaseDirectories;
use zeroize::Zeroizing;

use crate::schema;

const KEY_BYTES: usize = 32;
const CREDENTIAL_SERVICE: &str = "com.1fach.einfach-mail";

pub(super) struct OpenDatabase {
    pub(super) client: Client,
    pub(super) database_path: PathBuf,
    pub(super) database_identity: DatabaseIdentity,
    pub(super) lifecycle_lock_path: PathBuf,
    pub(super) credential_account: String,
    pub(super) account_lock: Arc<File>,
}

struct CachePaths {
    database: PathBuf,
    account_lock: PathBuf,
    lifecycle_lock: PathBuf,
    credential_account: String,
}

/// Holds the lifecycle gate while an open cache attempts account removal.
pub(super) struct AccountRemovalLock {
    account_lock: Arc<File>,
    _lifecycle_lock: File,
    restore_shared_on_drop: bool,
}

impl AccountRemovalLock {
    /// Acquires the lifecycle gate and attempts to make the open cache exclusive.
    ///
    /// # Errors
    ///
    /// Returns [`MailCacheError::InUse`] when another cache handle or lifecycle operation
    /// prevents removal.
    /// Returns [`MailCacheError::Unavailable`] when lock state cannot be restored.
    pub(super) fn acquire(
        lifecycle_lock_path: &Path,
        account_lock: Arc<File>,
    ) -> Result<Self, MailCacheError> {
        let lifecycle_lock = lock_account_directory_exclusive(lifecycle_lock_path)?;
        let mut removal_lock = Self {
            account_lock,
            _lifecycle_lock: lifecycle_lock,
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
    /// Returns [`MailCacheError::InUse`] when the account lock cannot be reacquired.
    /// Returns [`MailCacheError::Unavailable`] when shared ownership cannot be restored.
    pub(super) fn restore_shared(mut self) -> Result<(), MailCacheError> {
        restore_shared_account_lock(&self.account_lock)?;
        self.restore_shared_on_drop = false;
        Ok(())
    }

    /// Keeps exclusive ownership after successful removal until the cache handle drops.
    pub(super) fn retain_exclusive(mut self) {
        self.restore_shared_on_drop = false;
    }
}

impl Drop for AccountRemovalLock {
    fn drop(&mut self) {
        if self.restore_shared_on_drop {
            let _ = restore_shared_account_lock(&self.account_lock);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DatabaseIdentity {
    device: u64,
    inode: u64,
}

impl DatabaseIdentity {
    pub(super) fn read(path: &Path) -> Result<Self, MailCacheError> {
        let metadata = fs::metadata(path).map_err(|_| MailCacheError::Unavailable)?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    pub(super) fn matches_path(self, path: &Path) -> Result<bool, MailCacheError> {
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(_) => return Err(MailCacheError::Unavailable),
        };
        Ok(self.device == metadata.dev() && self.inode == metadata.ino())
    }
}

struct DatabaseKey(Zeroizing<[u8; KEY_BYTES]>);

impl DatabaseKey {
    fn generate() -> Result<Self, MailCacheError> {
        let mut bytes = Zeroizing::new([0_u8; KEY_BYTES]);
        getrandom::fill(bytes.as_mut()).map_err(|_| MailCacheError::Unavailable)?;
        Ok(Self(bytes))
    }

    fn from_secret(secret: Vec<u8>) -> Result<Self, MailCacheError> {
        let secret = Zeroizing::new(secret);
        if secret.len() != KEY_BYTES {
            return Err(MailCacheError::MissingEncryptionKey);
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

pub(super) async fn open_database(account_id: &AccountId) -> Result<OpenDatabase, MailCacheError> {
    let paths = cache_paths(account_id)?;
    let lifecycle_lock = lock_account_directory_shared(&paths.lifecycle_lock)?;
    let account_lock = Arc::new(lock_account_directory_shared(&paths.account_lock)?);
    drop(lifecycle_lock);
    let initialization_lock_path = with_suffix(&paths.account_lock, ".init");
    let _initialization_lock = lock_account_directory_exclusive(&initialization_lock_path)?;
    open_locked_database(paths, account_lock).await
}

async fn open_locked_database(
    paths: CachePaths,
    account_lock: Arc<File>,
) -> Result<OpenDatabase, MailCacheError> {
    let credential = credential_entry(&paths.credential_account)?;
    let key = load_or_create_key(&credential, encrypted_data_exists(&paths.database)?)?;
    open_database_with_key(paths, key, account_lock).await
}

async fn open_database_with_key(
    paths: CachePaths,
    key: DatabaseKey,
    account_lock: Arc<File>,
) -> Result<OpenDatabase, MailCacheError> {
    secure_database_file(&paths.database)?;
    let client = open_encrypted_client(&paths.database, &key).await?;
    schema::apply(&client)
        .await
        .map_err(super::store::CacheFailure::mail_public)?;
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
) -> Result<DatabaseKey, MailCacheError> {
    match credential.get_secret() {
        Ok(secret) => DatabaseKey::from_secret(secret),
        Err(KeyringError::NoEntry) if database_exists => Err(MailCacheError::MissingEncryptionKey),
        Err(KeyringError::NoEntry) => create_and_store_key(credential),
        Err(_) => Err(MailCacheError::Unavailable),
    }
}

fn create_and_store_key(credential: &Entry) -> Result<DatabaseKey, MailCacheError> {
    let key = DatabaseKey::generate()?;
    credential
        .set_secret(key.0.as_slice())
        .map_err(|_| MailCacheError::Unavailable)?;
    Ok(key)
}

async fn open_encrypted_client(
    database_path: &Path,
    key: &DatabaseKey,
) -> Result<Client, MailCacheError> {
    let client = ClientBuilder::new()
        .path(database_path)
        .open()
        .await
        .map_err(|_| MailCacheError::Unavailable)?;
    let key_statement = key.raw_key_statement();
    client
        .conn_mut(move |connection| connection.execute_batch(&key_statement))
        .await
        .map_err(|_| MailCacheError::Unavailable)?;
    client
        .conn_mut(|connection| connection.execute_batch("PRAGMA foreign_keys = ON"))
        .await
        .map_err(|_| MailCacheError::Unavailable)?;
    Ok(client)
}

#[cfg(test)]
pub(super) async fn open_test_database(
    database_path: &Path,
    key_byte: u8,
) -> Result<OpenDatabase, MailCacheError> {
    let account_directory = database_path.parent().ok_or(MailCacheError::Unavailable)?;
    let paths = CachePaths {
        database: database_path.to_owned(),
        account_lock: account_directory.join("mail-cache.lock"),
        lifecycle_lock: account_directory.join("mail-cache.lifecycle.lock"),
        credential_account: "synthetic-cache-key".to_owned(),
    };
    let lifecycle_lock = lock_account_directory_shared(&paths.lifecycle_lock)?;
    let account_lock = Arc::new(lock_account_directory_shared(&paths.account_lock)?);
    drop(lifecycle_lock);
    let initialization_lock_path = with_suffix(&paths.account_lock, ".init");
    let _initialization_lock = lock_account_directory_exclusive(&initialization_lock_path)?;
    let key = DatabaseKey(Zeroizing::new([key_byte; KEY_BYTES]));
    open_database_with_key(paths, key, account_lock).await
}

fn cache_paths(account_id: &AccountId) -> Result<CachePaths, MailCacheError> {
    let project = BaseDirectories::with_prefix("einfach-mail");
    let account_hash = hex::encode(Sha256::digest(account_id.as_str().as_bytes()));
    let account_directory = project
        .get_data_file(PathBuf::from("mail-cache").join(&account_hash))
        .ok_or(MailCacheError::Unavailable)?;
    Ok(CachePaths {
        database: account_directory.join("mail.sqlite3"),
        account_lock: account_directory.join("mail-cache.lock"),
        lifecycle_lock: account_directory.join("mail-cache.lifecycle.lock"),
        credential_account: format!("mail-cache-key-{account_hash}"),
    })
}

fn create_private_directory(path: &Path) -> Result<(), MailCacheError> {
    fs::create_dir_all(path).map_err(|_| MailCacheError::Unavailable)?;
    fs::set_permissions(path, Permissions::from_mode(0o700))
        .map_err(|_| MailCacheError::Unavailable)
}

fn open_lock_file(path: &Path) -> Result<File, MailCacheError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .map_err(|_| MailCacheError::Unavailable)?;
    file.set_permissions(Permissions::from_mode(0o600))
        .map_err(|_| MailCacheError::Unavailable)?;
    Ok(file)
}

fn secure_database_file(path: &Path) -> Result<(), MailCacheError> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .map_err(|_| MailCacheError::Unavailable)?
        .set_permissions(Permissions::from_mode(0o600))
        .map_err(|_| MailCacheError::Unavailable)
}

fn encrypted_data_exists(database_path: &Path) -> Result<bool, MailCacheError> {
    if path_exists(database_path)? {
        return Ok(true);
    }
    if path_exists(&with_suffix(database_path, "-wal"))? {
        return Ok(true);
    }
    path_exists(&with_suffix(database_path, "-shm"))
}

fn path_exists(path: &Path) -> Result<bool, MailCacheError> {
    path.try_exists().map_err(|_| MailCacheError::Unavailable)
}

pub(super) fn account_database_exists(account_id: &AccountId) -> Result<bool, MailCacheError> {
    encrypted_data_exists(&cache_paths(account_id)?.database)
}

pub(super) fn credential_entry(account: &str) -> Result<Entry, MailCacheError> {
    Entry::new(CREDENTIAL_SERVICE, account).map_err(|_| MailCacheError::Unavailable)
}

pub(super) fn remove_credential(account: &str) -> Result<(), MailCacheError> {
    match credential_entry(account)?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(_) => Err(MailCacheError::Unavailable),
    }
}

pub(super) async fn delete_database_files(
    client: &Client,
    database_path: &Path,
) -> Result<(), MailCacheError> {
    client
        .close()
        .await
        .map_err(|_| MailCacheError::Unavailable)?;
    remove_database_files(database_path)
}

pub(super) fn remove_account_without_open_client(
    account_id: &AccountId,
) -> Result<(), MailCacheError> {
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
    remove_credential: impl FnOnce() -> Result<(), MailCacheError>,
) -> Result<(), MailCacheError> {
    let _lifecycle_lock = lock_account_directory_exclusive(lifecycle_lock_path)?;
    let _account_lock = lock_account_directory_exclusive(account_lock_path)?;
    if encrypted_data_exists(database_path)? {
        remove_database_files(database_path)?;
    }
    remove_credential()
}

fn restore_shared_account_lock(file: &File) -> Result<(), MailCacheError> {
    file.unlock().map_err(|_| MailCacheError::Unavailable)?;
    file.try_lock_shared().map_err(lock_error)
}

/// Allows multiple cache sessions to share the account lock while excluding removal.
///
/// # Errors
///
/// Returns [`MailCacheError::InUse`] when an account removal owns the lock.
/// Returns [`MailCacheError::Unavailable`] when the lock file cannot be opened or locked.
pub(super) fn lock_account_directory_shared(path: &Path) -> Result<File, MailCacheError> {
    let file = open_account_lock_file(path)?;
    file.try_lock_shared().map_err(lock_error)?;
    Ok(file)
}

fn lock_account_directory_exclusive(path: &Path) -> Result<File, MailCacheError> {
    let file = open_account_lock_file(path)?;
    file.try_lock().map_err(lock_error)?;
    Ok(file)
}

fn open_account_lock_file(path: &Path) -> Result<File, MailCacheError> {
    let account_directory = path.parent().ok_or(MailCacheError::Unavailable)?;
    create_private_directory(account_directory)?;
    open_lock_file(path)
}

fn lock_error(error: std::fs::TryLockError) -> MailCacheError {
    match error {
        std::fs::TryLockError::WouldBlock => MailCacheError::InUse,
        std::fs::TryLockError::Error(_) => MailCacheError::Unavailable,
    }
}

fn remove_database_files(database_path: &Path) -> Result<(), MailCacheError> {
    // Remove sidecars first so a failure leaves the database available for retry.
    remove_file_if_present(&with_suffix(database_path, "-wal"))?;
    remove_file_if_present(&with_suffix(database_path, "-shm"))?;
    remove_file_if_present(database_path)
}

fn remove_file_if_present(path: &Path) -> Result<(), MailCacheError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(MailCacheError::Unavailable),
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(suffix);
    PathBuf::from(value)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };

    use super::{MailCacheError, open_test_database, remove_database_files, with_suffix};

    #[test]
    fn removing_a_cache_deletes_database_and_sidecars_idempotently() {
        let directory = tempfile::tempdir().expect("synthetic cache directory is created");
        let database = directory.path().join("mail.sqlite3");
        for path in [
            database.clone(),
            with_suffix(&database, "-wal"),
            with_suffix(&database, "-shm"),
        ] {
            fs::write(path, b"synthetic cache file").expect("synthetic cache file is created");
        }

        remove_database_files(&database).expect("cache files are removed");
        remove_database_files(&database).expect("repeated cache removal is safe");

        assert!(!database.exists());
        assert!(!with_suffix(&database, "-wal").exists());
        assert!(!with_suffix(&database, "-shm").exists());
    }

    #[test]
    fn removing_an_absent_account_cache_still_creates_its_lock() {
        let directory = tempfile::tempdir().expect("synthetic cache root is created");
        let account_directory = directory.path().join("account");
        let database_path = account_directory.join("mail.sqlite3");
        let lock_path = account_directory.join("mail-cache.lock");
        let lifecycle_lock_path = account_directory.join("mail-cache.lifecycle.lock");
        let credential_removed = Arc::new(AtomicBool::new(false));
        let observed_credential_removed = Arc::clone(&credential_removed);

        super::remove_unopened_account_cache(
            &database_path,
            &lock_path,
            &lifecycle_lock_path,
            move || {
                observed_credential_removed.store(true, Ordering::Relaxed);
                Ok(())
            },
        )
        .expect("the absent cache and credential can be removed");

        assert!(account_directory.is_dir());
        assert!(lock_path.is_file());
        assert!(lifecycle_lock_path.is_file());
        assert!(!database_path.exists());
        assert!(credential_removed.load(Ordering::Relaxed));
    }

    #[test]
    fn unopened_cache_removal_keeps_files_and_key_while_cache_is_open() {
        smol::block_on(async {
            let directory = tempfile::tempdir().expect("synthetic cache root is created");
            let account_directory = directory.path().join("account");
            let database_path = account_directory.join("mail.sqlite3");
            let lock_path = account_directory.join("mail-cache.lock");
            let lifecycle_lock_path = account_directory.join("mail-cache.lifecycle.lock");
            let opened = open_test_database(&database_path, 7)
                .await
                .expect("synthetic encrypted cache opens with its account lock");
            let second_opened = open_test_database(&database_path, 7)
                .await
                .expect("a second synthetic cache handle shares the account lock");
            let credential_removed = Arc::new(AtomicBool::new(false));
            let observed_credential_removed = Arc::clone(&credential_removed);

            assert_eq!(
                super::remove_unopened_account_cache(
                    &database_path,
                    &lock_path,
                    &lifecycle_lock_path,
                    move || {
                        observed_credential_removed.store(true, Ordering::Relaxed);
                        Ok(())
                    },
                ),
                Err(MailCacheError::InUse)
            );
            assert!(database_path.exists());
            assert!(!credential_removed.load(Ordering::Relaxed));

            second_opened
                .client
                .close()
                .await
                .expect("second synthetic database closes before retry");
            drop(second_opened);
            assert_eq!(
                super::remove_unopened_account_cache(
                    &database_path,
                    &lock_path,
                    &lifecycle_lock_path,
                    || Ok(()),
                ),
                Err(MailCacheError::InUse)
            );

            opened
                .client
                .close()
                .await
                .expect("synthetic database closes before retry");
            drop(opened);
            super::remove_unopened_account_cache(
                &database_path,
                &lock_path,
                &lifecycle_lock_path,
                || {
                    credential_removed.store(true, Ordering::Relaxed);
                    Ok(())
                },
            )
            .expect("removal succeeds after the cache handle closes");

            assert!(!database_path.exists());
            assert!(credential_removed.load(Ordering::Relaxed));
        });
    }

    #[test]
    fn failed_cache_lock_upgrade_restores_the_open_handle_lock() {
        smol::block_on(async {
            let directory = tempfile::tempdir().expect("synthetic cache root is created");
            let account_directory = directory.path().join("account");
            let database_path = account_directory.join("mail.sqlite3");
            let lifecycle_lock_path = account_directory.join("mail-cache.lifecycle.lock");
            let account_lock_path = account_directory.join("mail-cache.lock");
            let first = open_test_database(&database_path, 7)
                .await
                .expect("first synthetic cache handle opens");
            let second = open_test_database(&database_path, 7)
                .await
                .expect("second synthetic cache handle opens");

            assert_eq!(
                super::AccountRemovalLock::acquire(
                    &lifecycle_lock_path,
                    Arc::clone(&first.account_lock),
                )
                .map(|_| ()),
                Err(MailCacheError::InUse)
            );
            assert_eq!(
                super::remove_unopened_account_cache(
                    &database_path,
                    &account_lock_path,
                    &lifecycle_lock_path,
                    || Ok(()),
                ),
                Err(MailCacheError::InUse)
            );
            second
                .client
                .close()
                .await
                .expect("second synthetic database closes");
            drop(second);
            assert_eq!(
                super::remove_unopened_account_cache(
                    &database_path,
                    &account_lock_path,
                    &lifecycle_lock_path,
                    || Ok(()),
                ),
                Err(MailCacheError::InUse)
            );
            let removal_lock = super::AccountRemovalLock::acquire(
                &lifecycle_lock_path,
                Arc::clone(&first.account_lock),
            )
            .expect("cache lock upgrades after the other handle closes");
            assert!(matches!(
                open_test_database(&database_path, 7).await,
                Err(MailCacheError::InUse)
            ));
            removal_lock
                .restore_shared()
                .expect("cache lock returns to shared ownership after the check");
            let reopened = open_test_database(&database_path, 7)
                .await
                .expect("another cache handle opens after the lifecycle gate releases");
            reopened
                .client
                .close()
                .await
                .expect("reopened synthetic cache closes");
            drop(reopened);

            first
                .client
                .close()
                .await
                .expect("first synthetic database closes");
        });
    }

    #[test]
    fn cache_initialization_requires_its_exclusive_lock() {
        smol::block_on(async {
            let directory = tempfile::tempdir().expect("synthetic cache root is created");
            let account_directory = directory.path().join("account");
            let database_path = account_directory.join("mail.sqlite3");
            let lock_path = account_directory.join("mail-cache.lock");
            let initialization_lock_path = with_suffix(&lock_path, ".init");
            let initialization_lock =
                super::lock_account_directory_exclusive(&initialization_lock_path)
                    .expect("synthetic initialization lock is acquired");

            assert!(matches!(
                open_test_database(&database_path, 7).await,
                Err(MailCacheError::InUse)
            ));
            assert!(!database_path.exists());

            drop(initialization_lock);
            let opened = open_test_database(&database_path, 7)
                .await
                .expect("synthetic cache initializes after the lock is released");
            opened
                .client
                .close()
                .await
                .expect("synthetic database closes after initialization");
        });
    }

    #[test]
    fn cache_presence_check_fails_closed_when_a_parent_is_not_a_directory() {
        let directory = tempfile::tempdir().expect("synthetic cache directory is created");
        let blocked_parent = directory.path().join("blocked-parent");
        fs::write(&blocked_parent, b"synthetic non-directory parent")
            .expect("synthetic non-directory parent is created");
        let database = blocked_parent.join("mail.sqlite3");

        assert!(matches!(
            super::encrypted_data_exists(&database),
            Err(MailCacheError::Unavailable)
        ));
    }

    #[test]
    fn failed_sidecar_removal_keeps_the_encrypted_database_reopenable_for_retry() {
        smol::block_on(async {
            for failing_suffix in ["-wal", "-shm"] {
                let directory = tempfile::tempdir().expect("synthetic cache directory is created");
                let database = directory.path().join("mail.sqlite3");
                let opened = open_test_database(&database, 7)
                    .await
                    .expect("synthetic encrypted database opens");
                opened
                    .client
                    .close()
                    .await
                    .expect("synthetic database closes");
                drop(opened);

                if failing_suffix == "-shm" {
                    fs::write(with_suffix(&database, "-wal"), b"synthetic WAL")
                        .expect("synthetic WAL sidecar is created");
                }
                let failing_sidecar = with_suffix(&database, failing_suffix);
                fs::create_dir(&failing_sidecar)
                    .expect("synthetic invalid sidecar directory is created");

                assert!(matches!(
                    remove_database_files(&database),
                    Err(MailCacheError::Unavailable)
                ));
                assert!(database.exists(), "main database remains after failure");

                fs::remove_dir(&failing_sidecar)
                    .expect("synthetic filesystem condition is cleared");
                let retry = open_test_database(&database, 7)
                    .await
                    .expect("main encrypted database reopens for a retry");
                retry.client.close().await.expect("retry client closes");
                drop(retry);
                remove_database_files(&database).expect("retry removes the cache files");
                remove_database_files(&database).expect("repeated cache removal is safe");

                assert!(!database.exists());
                assert!(!with_suffix(&database, "-wal").exists());
                assert!(!with_suffix(&database, "-shm").exists());
            }
        });
    }
}
