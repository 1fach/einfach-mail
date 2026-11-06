# ADR 0038: Share mail cache handles by account

- Status: Accepted for Linux
- Date: 2026-11-06
- Owners: Project maintainer

## Context

`SqliteMailCacheFactory` opened a new SQLCipher client for every mail session. The account lock protected only database opening and migration, then its file handle was dropped. Removing the cache through one session could therefore unlink the database while a sibling session still held a live SQLite connection.

Mail sessions from one application factory share account and folder reservations. Their cache handles must also share one lifecycle so account removal closes every session that uses the account database.

## Decision

`SqliteMailCacheFactory` keeps a weak reference to one shared cache handle per account. Sessions opened by that factory reuse the same SQLCipher client. The cache handle retains the account's exclusive lock file for its lifetime, so a separate factory or process cannot open or remove that database concurrently.

Application workflows reserve the account before cache removal. Removal closes the shared client, deletes the database and SQLite sidecars, removes the Secret Service key, and releases the lock. If cleanup fails after the client closes, another session cannot reopen the incomplete cache; retrying removal continues the cleanup.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Keep independent SQLCipher clients and lock only opening or removal | Minimal adapter state | A live sibling client can read or write the unlinked database after another session removes it. |
| Retain a separate exclusive lock for each session | Each handle has a simple lifetime | Sessions for the same account would conflict, breaking the existing multi-session workflow contract. |
| Share one per-account client and lock lease | Keeps multi-session workflows while making removal close all handles and preventing cross-process races | Sessions from separate factory instances cannot independently open the same account cache while its lease is held. |

## Consequences

- One `SqliteMailCacheFactory` shares each account's client across its sessions.
- Cache opening and factory-level removal return `MailCacheError::OperationInProgress` when another process holds the account lock.
- Account removal is serialized with provider sync and body-fetch operations created by the same mail workflow factory.
- Linux tests verify that sibling sessions share a client, removal closes the client for every session, and a new cache can open after successful removal.

## Sources

- [System architecture](../architecture/system.md)
- [ADR 0012: Keep mail sessions behind application workflows](0012-application-owned-mail-sessions.md)
- [ADR 0037: Reject stale mail sync pages](0037-reject-stale-mail-sync-pages.md)
