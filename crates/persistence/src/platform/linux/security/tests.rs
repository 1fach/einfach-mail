mod production;

use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use super::{
    CacheLifecycleError, KeyLookup, classify_key_lookup, lock_account_directory_exclusive,
    lock_account_directory_shared, open_test_database, remove_database_files,
    take_lifecycle_lock_if_restore_failed, with_suffix,
};
use keyring::Error as KeyringError;

#[test]
fn keyring_lookup_distinguishes_new_existing_and_unavailable_states() {
    assert!(matches!(
        classify_key_lookup(Err(KeyringError::NoEntry), false),
        Ok(KeyLookup::Create)
    ));
    assert!(matches!(
        classify_key_lookup(Err(KeyringError::NoEntry), true),
        Ok(KeyLookup::Missing)
    ));
    assert!(matches!(
        classify_key_lookup(Ok(vec![0x31; super::KEY_BYTES]), true),
        Ok(KeyLookup::Existing(_))
    ));
    assert_eq!(
        classify_key_lookup(Ok(vec![0x31; super::KEY_BYTES - 1]), true).err(),
        Some(CacheLifecycleError::MissingEncryptionKey)
    );

    let locked_store_error = KeyringError::NoStorageAccess(Box::new(std::io::Error::other(
        "synthetic locked Secret Service",
    )));
    assert_eq!(
        classify_key_lookup(Err(locked_store_error), true).err(),
        Some(CacheLifecycleError::Unavailable)
    );
}

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
            Err(CacheLifecycleError::InUse)
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
            Err(CacheLifecycleError::InUse)
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
            Err(CacheLifecycleError::InUse)
        );
        assert_eq!(
            super::remove_unopened_account_cache(
                &database_path,
                &account_lock_path,
                &lifecycle_lock_path,
                || Ok(()),
            ),
            Err(CacheLifecycleError::InUse)
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
            Err(CacheLifecycleError::InUse)
        );
        let removal_lock = super::AccountRemovalLock::acquire(
            &lifecycle_lock_path,
            Arc::clone(&first.account_lock),
        )
        .expect("cache lock upgrades after the other handle closes");
        assert!(matches!(
            open_test_database(&database_path, 7).await,
            Err(CacheLifecycleError::InUse)
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
fn failed_shared_lock_restore_keeps_lifecycle_gate_held() {
    let directory = tempfile::tempdir().expect("synthetic cache root is created");
    let lifecycle_lock_path = directory.path().join("account/mail-cache.lifecycle.lock");
    let mut lifecycle_lock = Some(
        lock_account_directory_exclusive(&lifecycle_lock_path)
            .expect("synthetic lifecycle lock is acquired"),
    );

    let retained_lifecycle_lock = take_lifecycle_lock_if_restore_failed(
        Err(CacheLifecycleError::Unavailable),
        &mut lifecycle_lock,
    )
    .expect("failed restoration retains the lifecycle lock");

    assert!(lifecycle_lock.is_none());
    assert_eq!(
        lock_account_directory_shared(&lifecycle_lock_path).err(),
        Some(CacheLifecycleError::InUse)
    );

    drop(retained_lifecycle_lock);
    let shared_lock = lock_account_directory_shared(&lifecycle_lock_path)
        .expect("a cache open can proceed after the retained lock is released");
    drop(shared_lock);
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
            Err(CacheLifecycleError::InUse)
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
        Err(CacheLifecycleError::Unavailable)
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
                Err(CacheLifecycleError::Unavailable)
            ));
            assert!(database.exists(), "main database remains after failure");

            fs::remove_dir(&failing_sidecar).expect("synthetic filesystem condition is cleared");
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

fn amplify(spawn_children: bool) -> (usize, usize) {
    let stop = Arc::new(AtomicBool::new(false));
    let spawner_stop = Arc::clone(&stop);
    let spawner = std::thread::spawn(move || {
        let mut spawned = 0_usize;
        while spawn_children && !spawner_stop.load(Ordering::Relaxed) {
            std::process::Command::new("/bin/true").status().expect("child runs");
            spawned += 1;
        }
        spawned
    });
    let mut failures = 0;
    for _ in 0..200 {
        smol::block_on(async {
            let directory = tempfile::tempdir().expect("root");
            let database_path = directory.path().join("account/mail.sqlite3");
            let first = open_test_database(&database_path, 7).await.expect("first opens");
            match open_test_database(&database_path, 7).await {
                Ok(second) => {
                    second.client.close().await.expect("close");
                }
                Err(error) => {
                    eprintln!("second open failed: {error:?}");
                    failures += 1;
                }
            }
            first.client.close().await.expect("close");
        });
    }
    stop.store(true, Ordering::Relaxed);
    (failures, spawner.join().expect("spawner joins"))
}

#[test]
fn amplify_with_concurrent_child_spawns() {
    let (failures, spawned) = amplify(true);
    eprintln!("WITH SPAWNS: {failures}/200 second opens failed; {spawned} children spawned");
}

#[test]
fn amplify_without_child_spawns() {
    let (failures, _) = amplify(false);
    eprintln!("NO SPAWNS: {failures}/200 second opens failed");
}
