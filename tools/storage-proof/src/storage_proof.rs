use async_sqlite::{
    Client, ClientBuilder,
    rusqlite::{Connection, backup::Backup},
};
use rusqlite_migration::{M, Migrations};
#[cfg(target_os = "linux")]
use std::io;
use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use tempfile::TempDir;
use zeroize::Zeroizing;

type ProofError = Box<dyn Error + Send + Sync>;
type ProofResult<T> = Result<T, ProofError>;

const KEY_BYTES: usize = 32;
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";
const SYNTHETIC_ACTION: &str = "synthetic pending action";

struct DatabaseKey(Zeroizing<[u8; KEY_BYTES]>);

impl DatabaseKey {
    fn synthetic(byte: u8) -> Self {
        Self(Zeroizing::new([byte; KEY_BYTES]))
    }

    #[cfg(target_os = "linux")]
    fn generate() -> ProofResult<Self> {
        let mut key = Zeroizing::new([0; KEY_BYTES]);
        getrandom::fill(key.as_mut())?;
        Ok(Self(key))
    }

    #[cfg(target_os = "linux")]
    fn from_secret(secret: Vec<u8>) -> ProofResult<Self> {
        let secret = Zeroizing::new(secret);
        if secret.len() != KEY_BYTES {
            return Err(
                io::Error::new(io::ErrorKind::InvalidData, "invalid database key length").into(),
            );
        }

        let mut key = Zeroizing::new([0; KEY_BYTES]);
        key.as_mut().copy_from_slice(secret.as_slice());
        Ok(Self(key))
    }

    fn as_bytes(&self) -> &[u8; KEY_BYTES] {
        &self.0
    }

    fn sqlcipher_key_statement(&self) -> Zeroizing<String> {
        let encoded_key = Zeroizing::new(hex::encode(self.as_bytes()));
        let mut statement = Zeroizing::new(String::with_capacity(82));
        statement.push_str("PRAGMA key = \"x'");
        statement.push_str(&encoded_key);
        statement.push_str("'\"");
        statement
    }
}

struct EncryptedDatabase {
    client: Client,
    key: DatabaseKey,
}

async fn open_encrypted_database(path: &Path, key: DatabaseKey) -> ProofResult<EncryptedDatabase> {
    let client = ClientBuilder::new().path(path).open().await?;
    let statement = key.sqlcipher_key_statement();
    // SQLCipher needs raw-key syntax as a SQL literal; pragma_update would quote it as a passphrase.
    client
        .conn_mut(move |connection| connection.execute_batch(statement.as_str()))
        .await?;
    Ok(EncryptedDatabase { client, key })
}

fn proof_migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(
            "CREATE TABLE proof_pending_actions (
                id INTEGER PRIMARY KEY,
                payload TEXT NOT NULL
            );",
        ),
        M::up(
            "ALTER TABLE proof_pending_actions
             ADD COLUMN state TEXT NOT NULL DEFAULT 'pending';",
        ),
    ])
}

async fn apply_proof_migrations(client: &Client) -> ProofResult<()> {
    client
        .conn_mut_and_then(|connection| {
            proof_migrations()
                .to_latest(connection)
                .map_err(|error| Box::new(error) as ProofError)
        })
        .await?;
    Ok(())
}

async fn write_synthetic_action(client: &Client) -> ProofResult<()> {
    client
        .conn(|connection| {
            connection.execute(
                "INSERT INTO proof_pending_actions (id, payload) VALUES (1, ?1)",
                [SYNTHETIC_ACTION],
            )?;
            Ok(())
        })
        .await?;
    Ok(())
}

async fn read_synthetic_action(client: &Client) -> ProofResult<String> {
    let payload = client
        .conn(|connection| {
            connection.query_row(
                "SELECT payload FROM proof_pending_actions WHERE id = 1",
                [],
                |row| row.get(0),
            )
        })
        .await?;
    Ok(payload)
}

async fn schema_version(client: &Client) -> ProofResult<i64> {
    let version = client
        .conn(|connection| connection.query_row("PRAGMA user_version", [], |row| row.get(0)))
        .await?;
    Ok(version)
}

async fn create_encrypted_backup(
    source: &Client,
    backup_path: &Path,
    key: &DatabaseKey,
) -> ProofResult<()> {
    let backup_path = PathBuf::from(backup_path);
    let statement = key.sqlcipher_key_statement();
    source
        .conn_and_then(move |source_connection| -> ProofResult<()> {
            let mut destination = Connection::open(backup_path)?;
            // Apply the raw key before the online backup writes its first page.
            destination.execute_batch(statement.as_str())?;
            let backup = Backup::new(source_connection, &mut destination)?;
            backup.run_to_completion(64, Duration::from_millis(10), None)?;
            Ok(())
        })
        .await?;
    Ok(())
}

fn assert_database_file_is_encrypted(path: &Path) -> ProofResult<()> {
    let file_bytes = fs::read(path)?;
    assert!(file_bytes.len() >= SQLITE_HEADER.len());
    assert_ne!(&file_bytes[..SQLITE_HEADER.len()], SQLITE_HEADER);
    Ok(())
}

async fn create_database_with_synthetic_action(path: &Path, key_byte: u8) -> ProofResult<()> {
    let database = open_encrypted_database(path, DatabaseKey::synthetic(key_byte)).await?;
    apply_proof_migrations(&database.client).await?;
    assert_eq!(schema_version(&database.client).await?, 2);
    write_synthetic_action(&database.client).await?;
    database.client.close().await?;
    Ok(())
}

async fn assert_database_reopens_with_key(path: &Path, key_byte: u8) -> ProofResult<()> {
    let database = open_encrypted_database(path, DatabaseKey::synthetic(key_byte)).await?;
    apply_proof_migrations(&database.client).await?;
    assert_eq!(
        read_synthetic_action(&database.client).await?,
        SYNTHETIC_ACTION
    );
    database.client.close().await?;
    Ok(())
}

async fn assert_wrong_key_is_rejected(path: &Path, key_byte: u8) -> ProofResult<()> {
    let database = open_encrypted_database(path, DatabaseKey::synthetic(key_byte)).await?;
    let read_result = read_synthetic_action(&database.client).await;
    database.client.close().await?;
    assert!(read_result.is_err());
    Ok(())
}

#[test]
fn encrypted_database_migrates_reopens_and_rejects_a_wrong_key() -> ProofResult<()> {
    smol::block_on(async {
        let directory = TempDir::new()?;
        let database_path = directory.path().join("cache.sqlite3");
        create_database_with_synthetic_action(&database_path, 0x41).await?;
        assert_database_file_is_encrypted(&database_path)?;
        assert_database_reopens_with_key(&database_path, 0x41).await?;
        assert_wrong_key_is_rejected(&database_path, 0x42).await?;
        Ok(())
    })
}

async fn create_database_backup(
    database_path: &Path,
    backup_path: &Path,
    key_byte: u8,
) -> ProofResult<()> {
    let database = open_encrypted_database(database_path, DatabaseKey::synthetic(key_byte)).await?;
    apply_proof_migrations(&database.client).await?;
    write_synthetic_action(&database.client).await?;
    create_encrypted_backup(&database.client, backup_path, &database.key).await?;
    database.client.close().await?;
    Ok(())
}

async fn assert_backup_reopens_with_key(path: &Path, key_byte: u8) -> ProofResult<()> {
    let backup = open_encrypted_database(path, DatabaseKey::synthetic(key_byte)).await?;
    apply_proof_migrations(&backup.client).await?;
    assert_eq!(
        read_synthetic_action(&backup.client).await?,
        SYNTHETIC_ACTION
    );
    backup.client.close().await?;
    Ok(())
}

#[test]
fn encrypted_backup_can_be_reopened_with_the_original_key() -> ProofResult<()> {
    smol::block_on(async {
        let directory = TempDir::new()?;
        let database_path = directory.path().join("cache.sqlite3");
        let backup_path = directory.path().join("cache.backup.sqlite3");
        create_database_backup(&database_path, &backup_path, 0x31).await?;
        assert_database_file_is_encrypted(&backup_path)?;
        assert_backup_reopens_with_key(&backup_path, 0x31).await?;
        Ok(())
    })
}

#[cfg(target_os = "linux")]
mod credential_tests {
    use super::*;
    use keyring::{Entry, Error as KeyringError};
    use std::process::{Command, Output};

    const CREDENTIAL_SERVICE: &str = "com.einfach-mail.storage-proof";
    const UNAVAILABLE_SERVICE_CHILD: &str = "EINFACH_MAIL_STORAGE_PROOF_UNAVAILABLE_SERVICE_CHILD";

    struct TestCredential {
        username: String,
    }

    impl TestCredential {
        fn create() -> ProofResult<Self> {
            let mut random_id = [0_u8; 16];
            getrandom::fill(&mut random_id)?;
            let username = format!("storage-proof-{}", hex::encode(random_id));
            Ok(Self { username })
        }

        fn entry(&self) -> ProofResult<Entry> {
            Ok(Entry::new(CREDENTIAL_SERVICE, &self.username)?)
        }
    }

    impl Drop for TestCredential {
        fn drop(&mut self) {
            let entry = match self.entry() {
                Ok(entry) => entry,
                Err(error) => {
                    eprintln!(
                        "could not initialize isolated storage proof credential for cleanup: {error}"
                    );
                    return;
                }
            };
            match entry.delete_credential() {
                Ok(()) | Err(KeyringError::NoEntry) => {}
                Err(error) => {
                    eprintln!("could not clean up isolated storage proof credential: {error}");
                }
            }
        }
    }

    enum CacheOpen {
        Open(EncryptedDatabase),
        MissingKey,
    }

    enum CredentialKey {
        Existing(DatabaseKey),
        Create,
        Missing,
    }

    fn credential_key(
        encrypted_data_exists: bool,
        secret_result: keyring::Result<Vec<u8>>,
    ) -> ProofResult<CredentialKey> {
        match secret_result {
            Ok(secret) => Ok(CredentialKey::Existing(DatabaseKey::from_secret(secret)?)),
            Err(KeyringError::NoEntry) if encrypted_data_exists => Ok(CredentialKey::Missing),
            Err(KeyringError::NoEntry) => Ok(CredentialKey::Create),
            Err(error) => Err(Box::new(error)),
        }
    }

    fn encrypted_data_exists(database_path: &Path, backup_path: &Path) -> ProofResult<bool> {
        Ok(database_path.try_exists()? || backup_path.try_exists()?)
    }

    #[test]
    fn credential_lookup_distinguishes_new_and_existing_caches() -> ProofResult<()> {
        assert!(matches!(
            credential_key(false, Err(KeyringError::NoEntry))?,
            CredentialKey::Create
        ));
        assert!(matches!(
            credential_key(true, Err(KeyringError::NoEntry))?,
            CredentialKey::Missing
        ));
        let CredentialKey::Existing(existing_key) =
            credential_key(true, Ok(vec![0x31; KEY_BYTES]))?
        else {
            return Err(io::Error::other("valid key was not accepted").into());
        };
        assert_eq!(existing_key.as_bytes(), &[0x31; KEY_BYTES]);
        assert!(credential_key(true, Ok(vec![0x31; KEY_BYTES - 1])).is_err());
        Ok(())
    }

    #[test]
    fn backup_without_primary_still_requires_its_missing_credential() -> ProofResult<()> {
        let directory = TempDir::new()?;
        let database_path = directory.path().join("cache.sqlite3");
        let backup_path = directory.path().join("cache.backup.sqlite3");
        fs::write(&backup_path, b"synthetic encrypted backup")?;

        let has_encrypted_data = encrypted_data_exists(&database_path, &backup_path)?;
        assert!(!database_path.exists());
        assert!(has_encrypted_data);
        assert!(matches!(
            credential_key(has_encrypted_data, Err(KeyringError::NoEntry))?,
            CredentialKey::Missing
        ));
        assert!(backup_path.exists());
        Ok(())
    }

    async fn open_or_create_cache(
        database_path: &Path,
        backup_path: &Path,
        credential: &TestCredential,
    ) -> ProofResult<CacheOpen> {
        let credential_entry = credential.entry()?;
        let has_encrypted_data = encrypted_data_exists(database_path, backup_path)?;
        let key = match credential_key(has_encrypted_data, credential_entry.get_secret())? {
            CredentialKey::Existing(key) => key,
            CredentialKey::Missing => return Ok(CacheOpen::MissingKey),
            CredentialKey::Create => {
                let key = DatabaseKey::generate()?;
                credential_entry.set_secret(key.as_bytes())?;
                key
            }
        };
        Ok(CacheOpen::Open(
            open_encrypted_database(database_path, key).await?,
        ))
    }

    #[test]
    fn locked_credential_store_is_not_treated_as_a_missing_key() -> ProofResult<()> {
        let locked_error = KeyringError::NoStorageAccess(Box::new(io::Error::other(
            "synthetic locked Secret Service",
        )));
        let credential_lookup = credential_key(true, Err(locked_error));
        let error = credential_lookup
            .err()
            .ok_or_else(|| io::Error::other("locked credential store was accepted as a key"))?;

        assert!(matches!(
            error.downcast_ref::<KeyringError>(),
            Some(KeyringError::NoStorageAccess(_))
        ));
        Ok(())
    }

    fn remove_account_data(
        credential: &TestCredential,
        account_directory: &Path,
    ) -> ProofResult<()> {
        match credential.entry()?.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => {}
            Err(error) => return Err(Box::new(error)),
        }
        match fs::remove_dir_all(account_directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(Box::new(error)),
        }
        Ok(())
    }

    fn make_account_directory(parent: &TempDir) -> ProofResult<PathBuf> {
        let account_directory = parent.path().join("synthetic-account");
        fs::create_dir(&account_directory)?;
        Ok(account_directory)
    }

    async fn create_credentialed_cache(
        credential: &TestCredential,
        database_path: &Path,
        backup_path: &Path,
    ) -> ProofResult<()> {
        let CacheOpen::Open(database) =
            open_or_create_cache(database_path, backup_path, credential).await?
        else {
            return Err(io::Error::other("new cache unexpectedly had no key").into());
        };
        let stored_key = Zeroizing::new(credential.entry()?.get_secret()?);
        assert_eq!(stored_key.as_slice(), database.key.as_bytes());
        apply_proof_migrations(&database.client).await?;
        write_synthetic_action(&database.client).await?;
        create_encrypted_backup(&database.client, backup_path, &database.key).await?;
        database.client.close().await?;
        drop(database);
        assert_credentialed_cache_reopens(credential, database_path, backup_path).await?;
        assert_credentialed_cache_reopens(credential, backup_path, backup_path).await?;
        Ok(())
    }

    async fn assert_credentialed_cache_reopens(
        credential: &TestCredential,
        database_path: &Path,
        backup_path: &Path,
    ) -> ProofResult<()> {
        let CacheOpen::Open(database) =
            open_or_create_cache(database_path, backup_path, credential).await?
        else {
            return Err(io::Error::other("stored credential did not reopen the cache").into());
        };
        apply_proof_migrations(&database.client).await?;
        assert_eq!(
            read_synthetic_action(&database.client).await?,
            SYNTHETIC_ACTION
        );
        database.client.close().await?;
        drop(database);
        Ok(())
    }

    async fn assert_missing_key_preserves_encrypted_files(
        credential: &TestCredential,
        database_path: &Path,
        backup_path: &Path,
    ) -> ProofResult<()> {
        let database_before = fs::read(database_path)?;
        let backup_before = fs::read(backup_path)?;

        assert!(matches!(
            open_or_create_cache(database_path, backup_path, credential).await?,
            CacheOpen::MissingKey
        ));
        assert_file_unchanged(
            database_path,
            &database_before,
            "database changed while its key was missing",
        )?;
        assert_file_unchanged(
            backup_path,
            &backup_before,
            "backup changed while its key was missing",
        )?;
        assert_credential_is_missing(credential)?;
        Ok(())
    }

    fn assert_credential_is_missing(credential: &TestCredential) -> ProofResult<()> {
        assert!(matches!(
            credential.entry()?.get_secret(),
            Err(KeyringError::NoEntry)
        ));
        Ok(())
    }

    fn assert_file_unchanged(
        path: &Path,
        expected_bytes: &[u8],
        failure_message: &str,
    ) -> ProofResult<()> {
        assert_eq!(fs::read(path)?, expected_bytes, "{failure_message}");
        Ok(())
    }

    async fn assert_missing_key_preserves_primary_cache(
        credential: &TestCredential,
        database_path: &Path,
        backup_path: &Path,
    ) -> ProofResult<()> {
        create_credentialed_cache(credential, database_path, backup_path).await?;
        credential.entry()?.delete_credential()?;
        assert_missing_key_preserves_encrypted_files(credential, database_path, backup_path).await
    }

    async fn assert_missing_key_preserves_backup_without_primary(
        credential: &TestCredential,
        database_path: &Path,
        backup_path: &Path,
    ) -> ProofResult<()> {
        fs::remove_file(database_path)?;
        let backup_before_missing_key = fs::read(backup_path)?;
        assert!(matches!(
            open_or_create_cache(database_path, backup_path, credential).await?,
            CacheOpen::MissingKey
        ));
        assert!(!database_path.exists());
        assert_file_unchanged(
            backup_path,
            &backup_before_missing_key,
            "backup changed while its key was missing",
        )?;
        assert_credential_is_missing(credential)?;
        Ok(())
    }

    #[ignore = "requires a desktop Secret Service; run explicitly with --ignored"]
    #[test]
    fn missing_key_does_not_replace_a_key_or_open_pending_data() -> ProofResult<()> {
        smol::block_on(async {
            let temporary_directory = TempDir::new()?;
            let account_directory = make_account_directory(&temporary_directory)?;
            let database_path = account_directory.join("cache.sqlite3");
            let backup_path = account_directory.join("cache.backup.sqlite3");
            let credential = TestCredential::create()?;
            assert_missing_key_preserves_primary_cache(&credential, &database_path, &backup_path)
                .await?;
            assert_missing_key_preserves_backup_without_primary(
                &credential,
                &database_path,
                &backup_path,
            )
            .await?;
            remove_account_data(&credential, &account_directory)?;
            assert!(!account_directory.exists());
            Ok(())
        })
    }

    fn assert_cache_open_fails_without_secret_service(
        database_path: &Path,
        backup_path: &Path,
        credential: &TestCredential,
    ) -> ProofResult<()> {
        let error = smol::block_on(open_or_create_cache(database_path, backup_path, credential))
            .err()
            .ok_or_else(|| {
                io::Error::other("cache opened even though the Secret Service was unavailable")
            })?;
        if matches!(
            error.downcast_ref::<KeyringError>(),
            Some(KeyringError::NoDefaultStore)
        ) {
            Ok(())
        } else {
            Err(error)
        }
    }

    fn assert_existing_cache_is_preserved_without_secret_service(
        directory: &TempDir,
        credential: &TestCredential,
    ) -> ProofResult<()> {
        let database_path = directory.path().join("existing-cache.sqlite3");
        let backup_path = directory.path().join("existing-cache.backup.sqlite3");
        smol::block_on(create_database_backup(&database_path, &backup_path, 0x52))?;
        assert_database_file_is_encrypted(&database_path)?;
        assert_database_file_is_encrypted(&backup_path)?;
        let database_before = fs::read(&database_path)?;
        let backup_before = fs::read(&backup_path)?;

        assert_cache_open_fails_without_secret_service(&database_path, &backup_path, credential)?;

        assert_eq!(fs::read(database_path)?, database_before);
        assert_eq!(fs::read(backup_path)?, backup_before);
        Ok(())
    }

    fn assert_unavailable_secret_service_does_not_create_or_modify_a_cache() -> ProofResult<()> {
        let directory = TempDir::new()?;
        let database_path = directory.path().join("new-cache.sqlite3");
        let backup_path = directory.path().join("new-cache.backup.sqlite3");
        let credential = TestCredential::create()?;

        assert_cache_open_fails_without_secret_service(&database_path, &backup_path, &credential)?;
        assert!(!database_path.exists());
        assert!(!backup_path.exists());
        assert_existing_cache_is_preserved_without_secret_service(&directory, &credential)?;
        Ok(())
    }

    fn run_with_unavailable_secret_service() -> ProofResult<()> {
        let directory = TempDir::new()?;
        let socket_path = directory.path().join("no-session-bus");
        let bus_address = format!("unix:path={}", socket_path.display());
        let child_test_output = Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "storage_proof::credential_tests::unavailable_secret_service_does_not_create_or_modify_a_cache",
            ])
            .env("DBUS_SESSION_BUS_ADDRESS", bus_address)
            .env(UNAVAILABLE_SERVICE_CHILD, "1")
            .output()?;
        verify_unavailable_service_child_run(&child_test_output)
    }

    fn verify_unavailable_service_child_run(child_test_output: &Output) -> ProofResult<()> {
        let child_stdout = String::from_utf8_lossy(&child_test_output.stdout);
        let test_started = child_stdout.contains("running 1 test");
        if !child_test_output.status.success() || !test_started {
            return Err(io::Error::other(format!(
                "isolated unavailable-service check did not pass: stdout: {}; stderr: {}",
                child_stdout,
                String::from_utf8_lossy(&child_test_output.stderr),
            ))
            .into());
        }
        Ok(())
    }

    #[test]
    fn unavailable_secret_service_does_not_create_or_modify_a_cache() -> ProofResult<()> {
        if std::env::var_os(UNAVAILABLE_SERVICE_CHILD).is_some() {
            return assert_unavailable_secret_service_does_not_create_or_modify_a_cache();
        }

        // Keyring initialization is process-global, so use a child with an isolated D-Bus address.
        run_with_unavailable_secret_service()
    }

    #[ignore = "requires a desktop Secret Service; run explicitly with --ignored"]
    #[test]
    fn account_removal_deletes_the_credential_and_is_idempotent() -> ProofResult<()> {
        smol::block_on(async {
            let temporary_directory = TempDir::new()?;
            let account_directory = make_account_directory(&temporary_directory)?;
            let database_path = account_directory.join("cache.sqlite3");
            let backup_path = account_directory.join("cache.backup.sqlite3");
            let credential = TestCredential::create()?;
            create_credentialed_cache(&credential, &database_path, &backup_path).await?;
            remove_account_data(&credential, &account_directory)?;
            assert_credential_is_missing(&credential)?;
            assert!(!account_directory.exists());
            remove_account_data(&credential, &account_directory)?;
            Ok(())
        })
    }
}
