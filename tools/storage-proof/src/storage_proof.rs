use async_sqlite::{
    Client, ClientBuilder,
    rusqlite::{Connection, backup::Backup},
};
use rusqlite_migration::{M, Migrations};
#[cfg(windows)]
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

    #[cfg(windows)]
    fn generate() -> ProofResult<Self> {
        let mut key = Zeroizing::new([0; KEY_BYTES]);
        getrandom::fill(key.as_mut())?;
        Ok(Self(key))
    }

    #[cfg(windows)]
    fn from_secret(secret: Vec<u8>) -> ProofResult<Self> {
        let secret = Zeroizing::new(secret);
        let key = <[u8; KEY_BYTES]>::try_from(secret.as_slice()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "invalid database key length")
        })?;
        Ok(Self(Zeroizing::new(key)))
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

#[cfg(windows)]
mod windows_credential_tests {
    use super::*;
    use keyring::{Entry, Error as KeyringError};

    const CREDENTIAL_SERVICE: &str = "com.einfach-mail.storage-proof";

    struct TestCredential(Entry);

    impl TestCredential {
        fn create() -> ProofResult<Self> {
            let mut random_id = [0_u8; 16];
            getrandom::fill(&mut random_id)?;
            let username = format!("storage-proof-{}", hex::encode(random_id));
            Ok(Self(Entry::new(CREDENTIAL_SERVICE, &username)?))
        }
    }

    impl Drop for TestCredential {
        fn drop(&mut self) {
            match self.0.delete_credential() {
                Ok(()) | Err(KeyringError::NoEntry) => {}
                Err(error) => {
                    eprintln!("could not clean up isolated storage proof credential: {error}")
                }
            }
        }
    }

    enum CacheOpen {
        Open(EncryptedDatabase),
        MissingKey,
    }

    async fn open_or_create_cache(path: &Path, credential: &Entry) -> ProofResult<CacheOpen> {
        let database_exists = path.try_exists()?;
        let key = match credential.get_secret() {
            Ok(secret) => DatabaseKey::from_secret(secret)?,
            Err(KeyringError::NoEntry) if database_exists => return Ok(CacheOpen::MissingKey),
            Err(KeyringError::NoEntry) => {
                let key = DatabaseKey::generate()?;
                credential.set_secret(key.as_bytes())?;
                key
            }
            Err(error) => return Err(Box::new(error)),
        };
        Ok(CacheOpen::Open(open_encrypted_database(path, key).await?))
    }

    async fn remove_account_data(credential: &Entry, account_directory: &Path) -> ProofResult<()> {
        match credential.delete_credential() {
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
        credential: &Entry,
        database_path: &Path,
        backup_path: &Path,
    ) -> ProofResult<()> {
        let CacheOpen::Open(database) = open_or_create_cache(database_path, credential).await?
        else {
            return Err(io::Error::other("new cache unexpectedly had no key").into());
        };
        let stored_key = Zeroizing::new(credential.get_secret()?);
        assert!(stored_key.as_slice() == database.key.as_bytes());
        apply_proof_migrations(&database.client).await?;
        write_synthetic_action(&database.client).await?;
        create_encrypted_backup(&database.client, backup_path, &database.key).await?;
        database.client.close().await?;
        drop(database);
        assert_credentialed_cache_reopens(credential, database_path).await?;
        assert_credentialed_cache_reopens(credential, backup_path).await?;
        Ok(())
    }

    async fn assert_credentialed_cache_reopens(
        credential: &Entry,
        database_path: &Path,
    ) -> ProofResult<()> {
        let CacheOpen::Open(database) = open_or_create_cache(database_path, credential).await?
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
        credential: &Entry,
        database_path: &Path,
        backup_path: &Path,
    ) -> ProofResult<()> {
        assert!(matches!(
            open_or_create_cache(database_path, credential).await?,
            CacheOpen::MissingKey
        ));
        assert!(database_path.exists());
        assert!(backup_path.exists());
        assert!(matches!(
            credential.get_secret(),
            Err(KeyringError::NoEntry)
        ));
        Ok(())
    }

    #[test]
    fn missing_key_does_not_replace_a_key_or_open_pending_data() -> ProofResult<()> {
        smol::block_on(async {
            let temporary_directory = TempDir::new()?;
            let account_directory = make_account_directory(&temporary_directory)?;
            let database_path = account_directory.join("cache.sqlite3");
            let backup_path = account_directory.join("cache.backup.sqlite3");
            let credential = TestCredential::create()?;
            create_credentialed_cache(&credential.0, &database_path, &backup_path).await?;
            credential.0.delete_credential()?;
            assert_missing_key_preserves_encrypted_files(
                &credential.0,
                &database_path,
                &backup_path,
            )
            .await?;
            remove_account_data(&credential.0, &account_directory).await?;
            assert!(!account_directory.exists());
            Ok(())
        })
    }

    #[test]
    fn account_removal_deletes_the_credential_and_is_idempotent() -> ProofResult<()> {
        smol::block_on(async {
            let temporary_directory = TempDir::new()?;
            let account_directory = make_account_directory(&temporary_directory)?;
            let database_path = account_directory.join("cache.sqlite3");
            let backup_path = account_directory.join("cache.backup.sqlite3");
            let credential = TestCredential::create()?;
            create_credentialed_cache(&credential.0, &database_path, &backup_path).await?;
            remove_account_data(&credential.0, &account_directory).await?;
            assert!(matches!(
                credential.0.get_secret(),
                Err(KeyringError::NoEntry)
            ));
            assert!(!account_directory.exists());
            remove_account_data(&credential.0, &account_directory).await?;
            Ok(())
        })
    }
}
