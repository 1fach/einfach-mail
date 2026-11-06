# ADR 0029: Keep cache removal retryable after a sidecar failure

- Status: Accepted
- Date: 2026-11-05
- Owners: Einfach Mail maintainers

## Context

The desktop removes an account's encrypted cache before asking the provider to delete its Microsoft refresh token. The persistence adapter closes the SQLCipher client, removes the database files, and removes the cache key only after file removal succeeds. The provider clears its private session only after refresh-token deletion succeeds, and the account workflow clears its profile only after provider sign-out succeeds.

`security::remove_database_files` removed the main database before its `-wal` and `-shm` sidecars. If either sidecar could not be removed, the cache key remained but the database needed for a safe retry was gone. Removing the sidecars first keeps the encrypted database available until all sidecar cleanup succeeds.

A later source review found that cache-presence checks used `Path::exists()`, which hides filesystem inspection errors. An inaccessible account directory could therefore look absent, causing no-open-client sign-out to skip file cleanup and remove the existing key.

A review of concurrent removal found that the account lock was released after cache initialization. A no-open-client removal could then acquire the lock and delete files while another cache handle still used them.

Linux `flock` does not guarantee an atomic shared-to-exclusive conversion. A failed conversion can leave an open handle without its shared lock. A removal attempt must prevent new cache opens during conversion and restore shared ownership before releasing that protection.

SQLite normally checkpoints the write-ahead log and removes its sidecars when the last connection closes. The persistence adapter still removes any remaining sidecars after close. See [SQLite write-ahead logging](https://www.sqlite.org/wal.html).

## Decision

- Close the SQLCipher client before removing cache files.
- Remove the `-wal` and `-shm` sidecars before removing the main database.
- Remove the main database only after both sidecar removals succeed.
- Treat only a confirmed missing path as absent. Propagate other filesystem inspection errors before creating or removing the cache key.
- Hold a shared account lock for the lifetime of every open cache handle. Multiple sessions may hold this lock together, and cache clones share the same file handle.
- Acquire a separate shared lifecycle lock while opening a cache and taking its account lock. Removal holds the lifecycle lock exclusively while it tries to acquire the account lock exclusively. This gate prevents new opens while Linux `flock` converts the current shared lock; if another cache still holds a shared lock, restore the current handle's shared lock before releasing the gate.
- Hold a separate exclusive initialization lock while reading or creating the cache key, opening SQLCipher, and applying schema migrations. Release it when initialization completes so later sessions can open under shared lifetime locks.
- Require an exclusive account lock for cache removal. Removal through an open handle upgrades its shared lock while holding the lifecycle gate; no-open removal acquires a new exclusive lock. Return `MailCacheError::InUse` before closing SQLCipher or changing files or credentials when another handle still owns a shared lock.
- Use a non-blocking lock attempt when opening a cache or removing it. Opening fails with `MailCacheError::InUse` while an exclusive removal owns the lock.
- Keep the cache key and Microsoft refresh token when cache-file removal fails. Return the existing cache error so the user can repair the filesystem condition and retry.
- Keep the recovery test synthetic. Do not use real account data or Graph requests.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Remove sidecars before the main database | A sidecar error leaves the encrypted database available for retry. It changes only deletion order. | A filesystem error can still prevent sign-out until the path is usable. |
| Delete the main database first | The cache's primary file disappears early. | A sidecar error leaves the cache impossible to reopen for a safe retry. |
| Add a durable removal-intent record | Can let a restart finish deletion without reopening the cache. | Adds persistent lifecycle state and recovery logic for an error avoided by keeping the main file until the end. |
| Hold an exclusive account lock only while opening or removing files | Keeps the lock short-lived and simple. | Does not protect a cache handle after initialization; another removal can delete its files and key. |
| Upgrade the lifetime lock without a lifecycle gate | Uses one persistent lock file and a short removal path. | A failed Linux `flock` conversion can drop the caller's shared lock and race a new cache open. |
| Hold a shared lock while each cache handle is open and require an exclusive lock for removal | Allows multiple sessions to share a cache while preventing removal until all sessions close. | Requires a lifecycle gate around open and removal so a failed upgrade can restore shared ownership before new sessions open. |
| Use the lifetime lock to serialize initialization too | Requires only one lock file and one locking contract. | Prevents multiple sessions from opening an already initialized cache together. |

## Consequences

- Cache removal leaves the encrypted database and key in place if either sidecar cannot be removed.
- Filesystem inspection errors stop cache opening and sign-out before the key can be created or deleted.
- Each open cache handle retains a shared account lock until its final clone is dropped. Multiple sessions can open together. The lifecycle gate serializes opens with removals, so a failed `flock` conversion can restore the open handle's shared lock before other opens proceed. No-open removal and live-handle removal return `InUse` without changing files or credentials while another session remains open.
- Cache key setup and migrations run under a separate exclusive initialization lock, preventing simultaneous first opens from storing different keys or applying the schema concurrently.
- After the filesystem problem is cleared, the encrypted database can be reopened and removal retried.
- If provider credential deletion fails, the saved credential, active provider session, and account profile remain available for another sign-out attempt.
- Missing files remain safe to remove repeatedly.
- This decision does not change Graph behavior or credential-store policy.
- [Issue #44](https://github.com/1fach/einfach-mail/issues/44) tracks implementation and synthetic recovery proof.

## Sources

- [SQLite write-ahead logging](https://www.sqlite.org/wal.html)
- [Rust `File` lock methods](https://doc.rust-lang.org/std/fs/struct.File.html#method.try_lock): behavior is platform-dependent when a file handle already holds a lock.
- [Linux `flock(2)`](https://man7.org/linux/man-pages/man2/flock.2.html): lock conversion is not guaranteed to be atomic.
- `crates/persistence/src/security.rs`: SQLCipher file removal
- `crates/persistence/src/store.rs`: account cache ownership and removal
- [Issue #44: Make cache removal retryable after sidecar deletion fails](https://github.com/1fach/einfach-mail/issues/44)
