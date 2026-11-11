use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use einfach_mail_application::{AccountCacheOpenError, MailCache};
use einfach_mail_domain::{AccountId, FolderId, FolderReference, MailFolder, MessageId};
use keyring::{Entry, Error as KeyringError};
use tempfile::TempDir;
use zeroize::Zeroizing;

use crate::SqliteAccountCache;

use super::{KEY_BYTES, cache_paths, credential_entry, remove_credential, with_suffix};

const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";
const CHILD_MODE: &str = "EINFACH_MAIL_PERSISTENCE_TEST_CHILD";
const ACCOUNT_ID: &str = "EINFACH_MAIL_PERSISTENCE_TEST_ACCOUNT_ID";
const NEW_ACCOUNT_ID: &str = "EINFACH_MAIL_PERSISTENCE_NEW_ACCOUNT_ID";
const EXISTING_ACCOUNT_ID: &str = "EINFACH_MAIL_PERSISTENCE_EXISTING_ACCOUNT_ID";
const CACHE_LIFECYCLE_TEST: &str =
    "platform::linux::security::production_tests::production_secret_service_cache_lifecycle";
const UNAVAILABLE_SERVICE_TEST: &str = "platform::linux::security::production_tests::unavailable_secret_service_preserves_production_caches";
const LEGACY_SQLCIPHER_FIXTURE: &[u8] =
    include_bytes!("../../../../../tests/fixtures/legacy-sqlcipher-4.14-v1.db");

struct ProductionCache {
    account_id: AccountId,
    database_path: PathBuf,
    credential: Entry,
}

impl ProductionCache {
    fn new(account_id: AccountId) -> Self {
        let paths = cache_paths(&account_id).expect("production cache paths are available");
        let credential = credential_entry(&paths.credential_account)
            .expect("production cache credential entry is available");
        Self {
            account_id,
            database_path: paths.database,
            credential,
        }
    }

    fn create_and_store_folder(&self) {
        let cache = smol::block_on(SqliteAccountCache::open_for_account(
            self.account_id.clone(),
        ))
        .expect("production cache opens with the desktop Secret Service");
        let folder = synthetic_folder(&self.account_id);
        smol::block_on(MailCache::replace_folders(
            &cache,
            &self.account_id,
            vec![folder],
        ))
        .expect("synthetic folder is stored in the production cache");
        smol::block_on(cache.close_for_tests());
    }

    fn saved_key(&self) -> Zeroizing<Vec<u8>> {
        Zeroizing::new(
            self.credential
                .get_secret()
                .expect("production cache key is saved in Secret Service"),
        )
    }

    fn reject_wrong_key(&self, original_key: &[u8]) {
        let database_before = self.database_bytes();
        let wrong_key = Zeroizing::new(original_key.iter().map(|byte| !byte).collect::<Vec<_>>());
        self.credential
            .set_secret(wrong_key.as_slice())
            .expect("synthetic wrong key replaces the isolated Secret Service entry");
        assert!(
            smol::block_on(SqliteAccountCache::open_for_account(
                self.account_id.clone()
            ))
            .is_err()
        );
        assert_eq!(self.database_bytes(), database_before);
    }

    fn reject_malformed_key(&self) {
        let database_before = self.database_bytes();
        let malformed_key = Zeroizing::new(vec![0x31; KEY_BYTES - 1]);
        self.credential
            .set_secret(malformed_key.as_slice())
            .expect("synthetic malformed key replaces the isolated Secret Service entry");
        assert!(matches!(
            smol::block_on(SqliteAccountCache::open_for_account(
                self.account_id.clone()
            )),
            Err(AccountCacheOpenError::MissingEncryptionKey)
        ));
        assert_eq!(self.database_bytes(), database_before);
        let stored_key = self.saved_key();
        assert_eq!(stored_key.len(), KEY_BYTES - 1);
    }

    fn restore_key(&self, key: &[u8]) {
        self.credential
            .set_secret(key)
            .expect("original synthetic key is restored");
    }

    fn assert_reopens_with_saved_folder(&self) {
        let cache = smol::block_on(SqliteAccountCache::open_for_account(
            self.account_id.clone(),
        ))
        .expect("production cache reopens with its saved key");
        let folders = smol::block_on(MailCache::list_folders(&cache, &self.account_id))
            .expect("production cache reads the saved folder");
        assert_eq!(folders, vec![synthetic_folder(&self.account_id)]);
        smol::block_on(cache.close_for_tests());
    }

    fn reject_missing_key_without_changing_cache(&self) {
        let database_before = self.database_bytes();
        self.credential
            .delete_credential()
            .expect("synthetic cache key is removed to simulate key loss");
        assert!(matches!(
            smol::block_on(SqliteAccountCache::open_for_account(
                self.account_id.clone()
            )),
            Err(AccountCacheOpenError::MissingEncryptionKey)
        ));
        assert_eq!(self.database_bytes(), database_before);
        assert_credential_is_missing(&self.credential);
    }

    fn remove_cache_and_key(&self, original_key: &[u8]) {
        self.restore_key(original_key);
        smol::block_on(SqliteAccountCache::remove_for_account(&self.account_id))
            .expect("production account removal deletes the cache and key");
        assert!(!self.database_path.exists());
        assert!(!with_suffix(&self.database_path, "-wal").exists());
        assert!(!with_suffix(&self.database_path, "-shm").exists());
        assert_credential_is_missing(&self.credential);
        smol::block_on(SqliteAccountCache::remove_for_account(&self.account_id))
            .expect("production account removal is idempotent");
    }

    fn database_bytes(&self) -> Vec<u8> {
        fs::read(&self.database_path).expect("production encrypted database bytes are readable")
    }
}

#[ignore = "requires an unlocked desktop Secret Service; run explicitly with --ignored"]
#[test]
fn production_secret_service_cache_lifecycle() {
    if env::var_os(CHILD_MODE).is_some() {
        run_production_secret_service_cache_lifecycle();
        return;
    }

    let xdg_data_home = TempDir::new().expect("isolated XDG data home is created");
    let account_id = synthetic_account_id();
    let output = run_lifecycle_child(xdg_data_home.path(), &account_id);
    let credential_cleanup =
        cache_paths(&account_id).and_then(|paths| remove_credential(&paths.credential_account));

    assert_child_test_succeeded(&output);
    credential_cleanup.expect("synthetic Secret Service credential is removed");
}

#[test]
fn unavailable_secret_service_preserves_production_caches() {
    if env::var_os(CHILD_MODE).is_some() {
        run_unavailable_secret_service_checks();
        return;
    }

    let xdg_data_home = TempDir::new().expect("isolated XDG data home is created");
    let new_account = synthetic_account_id();
    let existing_account = synthetic_account_id();
    let output =
        run_unavailable_service_child(xdg_data_home.path(), &new_account, &existing_account);
    assert_child_test_succeeded(&output);
}

#[test]
fn legacy_sqlcipher_cache_migrates_through_production_adapter() {
    let temporary_directory = TempDir::new().expect("temporary cache directory is created");
    let database_path = temporary_directory.path().join("legacy-cache.db");
    fs::write(&database_path, LEGACY_SQLCIPHER_FIXTURE)
        .expect("synthetic legacy SQLCipher fixture is copied");
    assert_database_is_encrypted(&database_path);

    let account_id = AccountId::try_from("synthetic-legacy-account".to_owned())
        .expect("synthetic account identifier is valid");
    let folder = FolderReference::new(
        account_id.clone(),
        FolderId::try_from("inbox".to_owned()).expect("synthetic folder identifier is valid"),
    );
    let message_id = MessageId::try_from("legacy-message".to_owned())
        .expect("synthetic message identifier is valid");
    let cache = smol::block_on(SqliteAccountCache::open_with_test_key(
        database_path.clone(),
        account_id.clone(),
        0xa5,
    ));

    let (message, body) = smol::block_on(MailCache::read_message(&cache, &folder, &message_id))
        .expect("legacy cached message is readable")
        .expect("legacy cached message exists");
    assert_eq!(
        message.metadata().subject(),
        Some("Synthetic legacy subject")
    );
    assert_eq!(
        body.expect("legacy cached message body exists").content(),
        "Synthetic legacy body"
    );
    assert_database_is_encrypted(&database_path);
    smol::block_on(cache.close_for_tests());

    let reopened_cache = smol::block_on(SqliteAccountCache::open_with_test_key(
        database_path.clone(),
        account_id,
        0xa5,
    ));
    let reopened_message = smol::block_on(MailCache::read_message(
        &reopened_cache,
        &folder,
        &message_id,
    ))
    .expect("migrated cached message is readable")
    .expect("migrated cached message exists");
    assert_eq!(
        reopened_message.0.metadata().subject(),
        Some("Synthetic legacy subject")
    );
    assert_database_is_encrypted(&database_path);
    smol::block_on(reopened_cache.close_for_tests());
}

fn run_production_secret_service_cache_lifecycle() {
    let account_id = account_id_from_environment(ACCOUNT_ID);
    let cache = ProductionCache::new(account_id);
    cache.create_and_store_folder();
    let original_key = cache.saved_key();
    assert_eq!(original_key.len(), KEY_BYTES);
    assert_database_is_encrypted(&cache.database_path);
    cache.reject_wrong_key(&original_key);
    cache.reject_malformed_key();
    cache.restore_key(&original_key);
    cache.assert_reopens_with_saved_folder();
    cache.reject_missing_key_without_changing_cache();
    cache.remove_cache_and_key(&original_key);
}

fn run_unavailable_secret_service_checks() {
    let new_account = account_id_from_environment(NEW_ACCOUNT_ID);
    reject_new_cache_without_secret_service(new_account);
    let existing_account = account_id_from_environment(EXISTING_ACCOUNT_ID);
    preserve_existing_cache_without_secret_service(existing_account);
}

fn reject_new_cache_without_secret_service(account_id: AccountId) {
    let paths = cache_paths(&account_id).expect("new production cache paths are available");
    assert!(!paths.database.exists());
    assert!(matches!(
        smol::block_on(SqliteAccountCache::open_for_account(account_id)),
        Err(AccountCacheOpenError::Unavailable)
    ));
    assert!(!paths.database.exists());
}

fn preserve_existing_cache_without_secret_service(account_id: AccountId) {
    let paths = cache_paths(&account_id).expect("existing production cache paths are available");
    create_test_cache_with_folder(&paths.database, &account_id);
    let database_before =
        fs::read(&paths.database).expect("existing encrypted database is readable");
    assert!(matches!(
        smol::block_on(SqliteAccountCache::open_for_account(account_id)),
        Err(AccountCacheOpenError::Unavailable)
    ));
    assert_eq!(
        fs::read(&paths.database)
            .expect("existing database remains readable after unavailable-service rejection"),
        database_before
    );
}

fn create_test_cache_with_folder(database_path: &Path, account_id: &AccountId) {
    let cache = smol::block_on(SqliteAccountCache::open_with_test_key(
        database_path.to_owned(),
        account_id.clone(),
        0x56,
    ));
    smol::block_on(MailCache::replace_folders(
        &cache,
        account_id,
        vec![synthetic_folder(account_id)],
    ))
    .expect("synthetic folder is stored before disabling Secret Service");
    smol::block_on(cache.close_for_tests());
}

fn run_lifecycle_child(xdg_data_home: &Path, account_id: &AccountId) -> Output {
    Command::new(env::current_exe().expect("persistence test binary is available"))
        .args(["--exact", CACHE_LIFECYCLE_TEST, "--ignored", "--nocapture"])
        .env("XDG_DATA_HOME", xdg_data_home)
        .env(CHILD_MODE, "secret-service")
        .env(ACCOUNT_ID, account_id.as_str())
        .output()
        .expect("Secret Service lifecycle test child starts")
}

fn run_unavailable_service_child(
    xdg_data_home: &Path,
    new_account: &AccountId,
    existing_account: &AccountId,
) -> Output {
    let missing_bus = xdg_data_home.join("no-session-bus");
    let bus_address = format!("unix:path={}", missing_bus.display());
    Command::new(env::current_exe().expect("persistence test binary is available"))
        .args(["--exact", UNAVAILABLE_SERVICE_TEST, "--nocapture"])
        .env("XDG_DATA_HOME", xdg_data_home)
        .env("DBUS_SESSION_BUS_ADDRESS", bus_address)
        .env(CHILD_MODE, "unavailable-service")
        .env(NEW_ACCOUNT_ID, new_account.as_str())
        .env(EXISTING_ACCOUNT_ID, existing_account.as_str())
        .output()
        .expect("unavailable Secret Service test child starts")
}

fn synthetic_account_id() -> AccountId {
    let mut random_id = [0_u8; 16];
    getrandom::fill(&mut random_id).expect("synthetic account identifier is generated");
    AccountId::try_from(format!("production-cache-{}", hex::encode(random_id)))
        .expect("synthetic account identifier is valid")
}

fn account_id_from_environment(variable: &str) -> AccountId {
    AccountId::try_from(env::var(variable).expect("synthetic account identifier is provided"))
        .expect("synthetic account identifier is valid")
}

fn synthetic_folder(account_id: &AccountId) -> MailFolder {
    let folder_id = FolderId::try_from("synthetic-inbox".to_owned())
        .expect("synthetic folder identifier is valid");
    let reference = FolderReference::new(account_id.clone(), folder_id);
    MailFolder::new(reference, None, "Synthetic Inbox".to_owned())
        .expect("synthetic mail folder is valid")
}

fn assert_database_is_encrypted(path: &Path) {
    let file_bytes = fs::read(path).expect("production database bytes are readable");
    assert!(file_bytes.len() >= SQLITE_HEADER.len());
    assert_ne!(&file_bytes[..SQLITE_HEADER.len()], SQLITE_HEADER);
}

fn assert_credential_is_missing(credential: &Entry) {
    match credential.get_secret() {
        Err(KeyringError::NoEntry) => {}
        Ok(secret) => {
            drop(Zeroizing::new(secret));
            panic!("synthetic cache key remains in Secret Service");
        }
        Err(_) => panic!("Secret Service could not confirm the synthetic key is missing"),
    }
}

fn assert_child_test_succeeded(output: &Output) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("running 1 test"),
        "isolated persistence test did not pass: stdout: {stdout}; stderr: {stderr}"
    );
}
