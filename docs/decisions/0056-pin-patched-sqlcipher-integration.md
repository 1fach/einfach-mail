# ADR 0056: Pin the patched SQLCipher integration while upstream releases catch up

- Status: Accepted
- Date: 2026-11-09
- Owners: Project maintainer

## Context

RUSTSEC-2026-0459 affects the workspace's bundled SQLCipher 4.14.0, which uses SQLite 3.51.3. SQLCipher 4.20.0 updates its SQLite baseline to 3.54.1. The maintained rusqlite integration for SQLCipher 4.20.0 is still an open draft because its optional WebAssembly backend awaits a matching `sqlite-wasm-rs` release.

The released `async-sqlite` 0.7.0 supports rusqlite 0.41. The released `rusqlite_migration` 2.6.0 still depends on rusqlite 0.40, while its 0.41 update is an open draft. Replacing the migration library with Refinery would add migration-history bookkeeping alongside the cache's existing `PRAGMA user_version` contract. Existing encrypted caches and parallel-schema repairs must continue to open without losing data.

## Decision

Use `async-sqlite` 0.7.0 and pin exact commits for rusqlite's SQLCipher 4.20.0 integration and `rusqlite_migration`'s rusqlite 0.41 compatibility. Issue #262 approves this as a one-time exception for the supported native Linux target because no released dependency path preserves the current migration API and cache format. Keep the existing `rusqlite_migration` API and `PRAGMA user_version` schema behavior. Keep `ffi-sqlite-wasm-rs` disabled because the application targets native Linux and does not build for WebAssembly.

Runtime tests report SQLCipher 4.20.0 and SQLite 3.54.1. The production adapter opens, migrates, reads, and reopens a synthetic SQLCipher 4.14.0 cache. The production Secret Service key lifecycle test passes with the new dependency stack in a private headless D-Bus session. The RUSTSEC-2026-0459 exception was removed, and the full Linux local gate passed `cargo audit --deny yanked` using cached RustSec database revision `ba4d1245f77987d28e7a7f119ed21ad844c11f57`.

Both upstream PRs remain drafts. The rusqlite PR's Linux checks currently stop at dependency resolution because `sqlite-wasm-rs` 0.6.3 is unpublished; the application does not enable that path. The migration PR reports no checks. Keep `Cargo.lock` committed so builds use the reviewed source revisions. Follow-up issue [#264](https://github.com/1fach/einfach-mail/issues/264) tracks replacing the pins with compatible official releases.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Pin the two upstream draft commits | Uses the maintained rusqlite SQLCipher integration and preserves the current migration API and cache format | Adds exact Git source pins until upstream publishes the changes; WebAssembly remains outside the supported build graph |
| Switch to Refinery 0.10 | Published support for rusqlite 0.41 | Replaces the current `user_version` migration record with a history table and requires an explicit legacy-cache import and reconciliation path |
| Keep the current crates and audit exception | No dependency or migration changes | Leaves SQLCipher on a SQLite version affected by the advisory |
| Maintain a local `libsqlite3-sys` fork or copied SQLCipher source | Controls the release schedule | Adds a long-term native-code patch to maintain and review |

## Consequences

- Persistence remains the only crate that owns SQLite, SQLCipher, and schema migration.
- The migration SQL and existing parallel-schema repairs stay unchanged.
- Builds depend on the pinned Git revisions until equivalent releases are available.
- The local verification covers Linux only. Windows and macOS remain deferred.
- The `sqlite-wasm-rs` path is not enabled or tested.

## Sources

- [SQLCipher 4.20.0 release notes](https://github.com/sqlcipher/sqlcipher/blob/v4.20.0/CHANGELOG.md)
- [rusqlite PR #1932: Bump bundled SQLCipher to 4.20.0](https://github.com/rusqlite/rusqlite/pull/1932)
- [rusqlite_migration PR #370: Bump rusqlite from 0.40 to 0.41](https://github.com/cljoly/rusqlite_migration/pull/370)
- [Issue #262: Update and verify the bundled SQLite source](https://github.com/1fach/einfach-mail/issues/262)
