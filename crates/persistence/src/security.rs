//! Linux cache paths, file locking, and SQLCipher key access.

use std::{
    fs::{self, File, OpenOptions, Permissions, TryLockError},
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
    pub(super) lock_file: File,
    pub(super) credential_account: Option<String>,
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
    let account_directory = database_path.parent().ok_or(MailCacheError::Unavailable)?;
    create_private_directory(account_directory)?;
    let lock_file = lock_account_directory(&lock_path)?;
    open_locked_database(database_path, lock_file, credential_account).await
}

async fn open_locked_database(
    database_path: PathBuf,
    lock_file: File,
    credential_account: String,
) -> Result<OpenDatabase, MailCacheError> {
    let credential = credential_entry(&credential_account)?;
    let key = load_or_create_key(&credential, encrypted_data_exists(&database_path))?;
    secure_database_file(&database_path)?;
    let client = open_encrypted_client(&database_path, &key).await?;
    schema::apply(&client)
        .await
        .map_err(super::store::CacheFailure::public)?;
    Ok(OpenDatabase {
        client,
        database_path,
        lock_file,
        credential_account: Some(credential_account),
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
    create_private_directory(account_directory)?;
    let lock_path = database_path.with_extension("lock");
    let lock_file = lock_account_directory(&lock_path)?;
    secure_database_file(database_path)?;
    let key = DatabaseKey(Zeroizing::new([key_byte; KEY_BYTES]));
    let client = open_encrypted_client(database_path, &key).await?;
    schema::apply(&client)
        .await
        .map_err(super::store::CacheFailure::public)?;
    Ok(OpenDatabase {
        client,
        database_path: database_path.to_path_buf(),
        lock_file,
        credential_account: None,
    })
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

fn encrypted_data_exists(database_path: &Path) -> bool {
    database_path.exists()
        || with_suffix(database_path, "-wal").exists()
        || with_suffix(database_path, "-shm").exists()
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

pub(super) fn remove_account_without_open_client(
    account_id: &AccountId,
) -> Result<(), MailCacheError> {
    let (database_path, lock_path, credential_account) = cache_paths(account_id)?;
    if let Some(account_directory) = database_path.parent()
        && account_directory.exists()
    {
        let _lock = lock_account_directory(&lock_path)?;
        remove_database_files(&database_path)?;
    }
    remove_credential(&credential_account)
}

pub(super) fn remove_database_files(database_path: &Path) -> Result<(), MailCacheError> {
    remove_file_if_present(database_path)?;
    remove_file_if_present(&with_suffix(database_path, "-wal"))?;
    remove_file_if_present(&with_suffix(database_path, "-shm"))
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
    let file = open_lock_file(path)?;
    file.try_lock().map_err(|error| match error {
        TryLockError::WouldBlock => MailCacheError::OperationInProgress,
        TryLockError::Error(_) => MailCacheError::Unavailable,
    })?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{remove_database_files, with_suffix};

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
}
