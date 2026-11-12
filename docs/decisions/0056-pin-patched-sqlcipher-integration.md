# ADR 0056: Pin the patched SQLCipher integration while upstream releases catch up

- Status: Accepted
- Date: 2026-11-09
- Owners: Project maintainer

## Context

RUSTSEC-2026-0459 affects the workspace's previous bundled SQLCipher 4.14.0, which uses SQLite 3.51.3. SQLCipher 4.20.0 updates its SQLite baseline to 3.54.1. The maintained integration is now released as rusqlite 0.42.0 with libsqlite3-sys 0.40.0 and builds for native Linux from registry packages.

The released `async-sqlite` 0.7.0 and `rusqlite_migration` 2.7.0 both depend on rusqlite 0.41. The migration crate now has a compatible registry release and replaces its former Git pin. The SQLCipher integration cannot yet move to rusqlite 0.42 without released async and migration versions that both support it. The registry libsqlite3-sys 0.39.1 used by rusqlite 0.41 still bundles SQLCipher 4.19.0. Replacing the migration library with Refinery would add migration-history bookkeeping alongside the cache's existing `PRAGMA user_version` contract. Existing encrypted caches and parallel-schema repairs must continue to open without losing data.

## Decision

Use `async-sqlite` 0.7.0 and `rusqlite_migration` 2.7.0 from crates.io. Keep one exact Git pin for rusqlite's SQLCipher 4.20.0 integration until released async and migration crates both support rusqlite 0.42. Issue #262 approves this remaining pin as a one-time exception for the supported native Linux target because no complete released dependency path currently preserves the existing adapter, migration API, and cache format. Keep the existing `PRAGMA user_version` schema behavior. Keep `ffi-sqlite-wasm-rs` disabled because the application targets native Linux and does not build for WebAssembly.

Runtime tests report SQLCipher 4.20.0 and SQLite 3.54.1. The production adapter opens, migrates, reads, and reopens a synthetic SQLCipher 4.14.0 cache. The production Secret Service key lifecycle test passes with the new dependency stack in a private headless D-Bus session. The RUSTSEC-2026-0459 exception was removed, and the full Linux local gate passed `cargo audit --deny yanked` using cached RustSec database revision `ba4d1245f77987d28e7a7f119ed21ad844c11f57`.

Rusqlite PR #1932 is now included in rusqlite 0.42.0, and `rusqlite_migration` 2.7.0 is a published release. A temporary Linux proof confirmed that rusqlite 0.42.0 builds with bundled SQLCipher from registry dependencies. The app still resolves rusqlite 0.41.1 through `async-sqlite` and `rusqlite_migration`, so the registry SQLCipher source is 4.19.0. This update removes the migration Git pin. Follow-up issue [#264](https://github.com/1fach/einfach-mail/issues/264) tracks removing the remaining SQLCipher pin when compatible async and migration releases support rusqlite 0.42. Keep `Cargo.lock` committed.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Pin rusqlite's SQLCipher integration while using published migration support | Uses the maintained rusqlite SQLCipher integration and preserves the current migration API and cache format | Keeps one exact Git source pin until the async and migration crates support the released rusqlite 0.42 line; WebAssembly remains outside the supported build graph |
| Switch to Refinery 0.10 | Published support for rusqlite 0.41 | Replaces the current `user_version` migration record with a history table and requires an explicit legacy-cache import and reconciliation path |
| Keep the current crates and audit exception | No dependency or migration changes | Leaves SQLCipher on a SQLite version affected by the advisory |
| Maintain a local `libsqlite3-sys` fork or copied SQLCipher source | Controls the release schedule | Adds a long-term native-code patch to maintain and review |

## Consequences

- Persistence remains the only crate that owns SQLite, SQLCipher, and schema migration.
- The migration SQL and existing parallel-schema repairs stay unchanged.
- Builds keep one pinned `libsqlite3-sys` Git revision until a compatible released dependency set is available.
- The local verification covers Linux only. Windows and macOS remain deferred.
- The `sqlite-wasm-rs` path is not enabled or tested.

## Sources

- [SQLCipher 4.20.0 release notes](https://github.com/sqlcipher/sqlcipher/blob/v4.20.0/CHANGELOG.md)
- [rusqlite PR #1932: Bump bundled SQLCipher to 4.20.0](https://github.com/rusqlite/rusqlite/pull/1932)
- [rusqlite 0.42.0 release](https://github.com/rusqlite/rusqlite/releases/tag/v0.42.0)
- [rusqlite_migration 2.7.0 release](https://github.com/cljoly/rusqlite_migration/releases/tag/v2.7.0)
- [async-sqlite 0.7.0 manifest](https://docs.rs/crate/async-sqlite/0.7.0/source/Cargo.toml.orig)
- [Issue #262: Update and verify the bundled SQLite source](https://github.com/1fach/einfach-mail/issues/262)
