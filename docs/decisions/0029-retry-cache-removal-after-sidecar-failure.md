# ADR 0029: Keep cache removal retryable after a sidecar failure

- Status: Accepted
- Date: 2026-11-01
- Owners: Einfach Mail maintainers

## Context

The desktop removes an account's encrypted cache before asking the provider to delete its Microsoft refresh token. The persistence adapter closes the SQLCipher client, removes the database files, and removes the cache key only after file removal succeeds. The provider clears its private session only after refresh-token deletion succeeds, and the account workflow clears its profile only after provider sign-out succeeds.

`security::remove_database_files` removed the main database before its `-wal` and `-shm` sidecars. If either sidecar could not be removed, the cache key remained but the database needed for a safe retry was gone. Removing the sidecars first keeps the encrypted database available until all sidecar cleanup succeeds.

A later source review found that cache-presence checks used `Path::exists()`, which hides filesystem inspection errors. An inaccessible account directory could therefore look absent, causing no-open-client sign-out to skip file cleanup and remove the existing key.

SQLite normally checkpoints the write-ahead log and removes its sidecars when the last connection closes. The persistence adapter still removes any remaining sidecars after close. See [SQLite write-ahead logging](https://www.sqlite.org/wal.html).

## Decision

- Close the SQLCipher client before removing cache files.
- Remove the `-wal` and `-shm` sidecars before removing the main database.
- Remove the main database only after both sidecar removals succeed.
- Treat only a confirmed missing path as absent. Propagate other filesystem inspection errors before creating or removing the cache key.
- Keep the cache key and Microsoft refresh token when cache-file removal fails. Return the existing cache error so the user can repair the filesystem condition and retry.
- Keep the recovery test synthetic. Do not use real account data or Graph requests.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Remove sidecars before the main database | A sidecar error leaves the encrypted database available for retry. It changes only deletion order. | A filesystem error can still prevent sign-out until the path is usable. |
| Delete the main database first | The cache's primary file disappears early. | A sidecar error leaves the cache impossible to reopen for a safe retry. |
| Add a durable removal-intent record | Can let a restart finish deletion without reopening the cache. | Adds persistent lifecycle state and recovery logic for an error avoided by keeping the main file until the end. |

## Consequences

- Cache removal leaves the encrypted database and key in place if either sidecar cannot be removed.
- Filesystem inspection errors stop cache opening and sign-out before the key can be created or deleted.
- After the filesystem problem is cleared, the encrypted database can be reopened and removal retried.
- If provider credential deletion fails, the saved credential, active provider session, and account profile remain available for another sign-out attempt.
- Missing files remain safe to remove repeatedly.
- This decision does not change Graph behavior or credential-store policy.
- [Issue #44](https://github.com/1fach/einfach-mail/issues/44) tracks implementation and synthetic recovery proof.

## Sources

- [SQLite write-ahead logging](https://www.sqlite.org/wal.html)
- `crates/persistence/src/security.rs`: SQLCipher file removal
- [Issue #44: Make cache removal retryable after sidecar deletion fails](https://github.com/1fach/einfach-mail/issues/44)
