//! Linux cache paths, file locking, and SQLCipher key access.

use std::{
    fs::{self, File, OpenOptions, Permissions},
    io,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
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
    pub(super) lock_path: PathBuf,
    pub(super) credential_account: String,
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
    let (database_path, lock_path, credential_account) = cache_paths(account_id)?;
    let _lock = lock_account_directory(&lock_path)?;
    open_locked_database(database_path, lock_path, credential_account).await
}

async fn open_locked_database(
    database_path: PathBuf,
    lock_path: PathBuf,
    credential_account: String,
) -> Result<OpenDatabase, MailCacheError> {
    let credential = credential_entry(&credential_account)?;
    let key = load_or_create_key(&credential, encrypted_data_exists(&database_path)?)?;
    secure_database_file(&database_path)?;
    let client = open_encrypted_client(&database_path, &key).await?;
    schema::apply(&client)
        .await
        .map_err(super::store::CacheFailure::public)?;
    Ok(OpenDatabase {
        client,
        database_path,
        lock_path,
        credential_account,
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
) -> Result<Client, MailCacheError> {
    secure_database_file(database_path)?;
    let key = DatabaseKey(Zeroizing::new([key_byte; KEY_BYTES]));
    let client = open_encrypted_client(database_path, &key).await?;
    schema::apply(&client)
        .await
        .map_err(super::store::CacheFailure::public)?;
    Ok(client)
}

fn cache_paths(account_id: &AccountId) -> Result<(PathBuf, PathBuf, String), MailCacheError> {
    let project = BaseDirectories::with_prefix("einfach-mail");
    let account_hash = hex::encode(Sha256::digest(account_id.as_str().as_bytes()));
    let account_directory = project
        .get_data_file(PathBuf::from("mail-cache").join(&account_hash))
        .ok_or(MailCacheError::Unavailable)?;
    let database_path = account_directory.join("mail.sqlite3");
    let lock_path = account_directory.join("mail-cache.lock");
    let credential_account = format!("mail-cache-key-{account_hash}");
    Ok((database_path, lock_path, credential_account))
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
    let (database_path, lock_path, credential_account) = cache_paths(account_id)?;
    remove_unopened_account_cache(&database_path, &lock_path, || {
        remove_credential(&credential_account)
    })
}

fn remove_unopened_account_cache(
    database_path: &Path,
    lock_path: &Path,
    remove_credential: impl FnOnce() -> Result<(), MailCacheError>,
) -> Result<(), MailCacheError> {
    let _lock = lock_account_directory(lock_path)?;
    if encrypted_data_exists(database_path)? {
        remove_database_files(database_path)?;
    }
    remove_credential()
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

pub(super) fn lock_account_directory(path: &Path) -> Result<File, MailCacheError> {
    let account_directory = path.parent().ok_or(MailCacheError::Unavailable)?;
    create_private_directory(account_directory)?;
    let file = open_lock_file(path)?;
    file.lock().map_err(|_| MailCacheError::Unavailable)?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        thread,
        time::Duration,
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
        let credential_removed = Arc::new(AtomicBool::new(false));
        let observed_credential_removed = Arc::clone(&credential_removed);

        super::remove_unopened_account_cache(&database_path, &lock_path, move || {
            observed_credential_removed.store(true, Ordering::Relaxed);
            Ok(())
        })
        .expect("the absent cache and credential can be removed");

        assert!(account_directory.is_dir());
        assert!(lock_path.is_file());
        assert!(!database_path.exists());
        assert!(credential_removed.load(Ordering::Relaxed));
    }

    #[test]
    fn unopened_cache_removal_waits_for_an_open_database_before_removing_credential() {
        let directory = tempfile::tempdir().expect("synthetic cache root is created");
        let account_directory = directory.path().join("account");
        let database_path = account_directory.join("mail.sqlite3");
        let lock_path = account_directory.join("mail-cache.lock");
        let open_lock = super::lock_account_directory(&lock_path)
            .expect("the synthetic open operation holds the account lock");
        fs::write(&database_path, b"synthetic encrypted cache")
            .expect("the synthetic open operation creates its database");

        let (started_sender, started_receiver) = mpsc::channel();
        let (credential_sender, credential_receiver) = mpsc::channel();
        let credential_removed = Arc::new(AtomicBool::new(false));
        let observed_credential_removed = Arc::clone(&credential_removed);
        let removal = thread::spawn(move || {
            started_sender
                .send(())
                .expect("the test observes the removal attempt");
            super::remove_unopened_account_cache(&database_path, &lock_path, move || {
                observed_credential_removed.store(true, Ordering::Relaxed);
                credential_sender
                    .send(())
                    .expect("the test observes credential removal");
                Ok(())
            })
        });

        started_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("the removal attempt starts while the open lock is held");
        assert!(matches!(
            credential_receiver.recv_timeout(Duration::from_millis(25)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        drop(open_lock);
        let result = removal.join().expect("the cache removal thread completes");

        result.expect("the cache is removed after its open lock is released");
        credential_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("credential removal follows cache file removal");
        assert!(!account_directory.join("mail.sqlite3").exists());
        assert!(credential_removed.load(Ordering::Relaxed));
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
                let client = open_test_database(&database, 7)
                    .await
                    .expect("synthetic encrypted database opens");
                client.close().await.expect("synthetic database closes");

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
                retry.close().await.expect("retry client closes");
                remove_database_files(&database).expect("retry removes the cache files");
                remove_database_files(&database).expect("repeated cache removal is safe");

                assert!(!database.exists());
                assert!(!with_suffix(&database, "-wal").exists());
                assert!(!with_suffix(&database, "-shm").exists());
            }
        });
    }
}
