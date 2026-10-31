# ADR 0005: Use SQLCipher-backed SQLite for the local cache

- Status: Proposed; Windows and key-lifecycle proof required
- Date: 2026-10-31
- Owners: Project maintainer

## Context

ADR 0004 requires durable offline mail and calendar data, with Graph changes and their sync cursor committed atomically. That cache needs relational queries for folders, message lists, calendar ranges, and pending operations. The product also requires an at-rest encryption decision before real mailbox data is stored.

The app is a native Rust desktop application. A pure-Rust UI and application stack is preferred, but the database choice must also provide mature relational behavior and a maintained encryption implementation. The first support target is Windows.

## Decision

Prefer SQLCipher-backed SQLite behind the planned `crates/persistence/` boundary. The application and domain crates will not depend on rusqlite or SQLCipher types.

The leading dependency set is `rusqlite` 0.40.2 with bundled SQLCipher and `rusqlite_migration` 2.6.0 for schema migrations. The migration crate uses SQLite's `user_version`, applies migrations atomically, supports rusqlite 0.40, and requires Rust 1.95, which matches the workspace. It has a smaller SQLite-specific dependency surface than Refinery 0.10.0, which supports rusqlite 0.40 and Rust 1.92 but also provides other database backends and migration macros. Prefer `rusqlite_migration` unless the storage spike shows a need for Refinery's broader features. Recheck the resolved dependency graph and RustSec advisories before adding either library. The worker or async wrapper is still open; evaluate a maintained executor-agnostic rusqlite wrapper against the GPUI executor before adding one.

Use an operating-system credential store for the database encryption key. The key lifecycle, missing-key recovery, cache backup, and account-removal deletion behavior remain open, so this ADR is not accepted and no real mailbox data may be stored yet.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| SQLCipher-backed SQLite through rusqlite | Relational queries, transactions, bundled encryption, mature SQLite ecosystem; Rust API stays behind the persistence adapter | SQLCipher and OpenSSL add native C build and licensing inputs. Windows x64 encrypted read/write and credential-store behavior need proof. |
| SQLx with SQLite | Async query API, migrations, supports SQLite and server databases | The documented SQLite feature set does not offer a supported SQLCipher configuration. The SQLite driver also uses `libsqlite3-sys`, which must be coordinated with any rusqlite dependency. |
| redb | Pure Rust, ACID transactions, concurrent readers | Key-value model increases query/index work for mail and calendar; the project docs do not provide database encryption. |
| Turso | In-process relational engine implemented in Rust and compatible with much of SQLite | The project documents beta status and missing SQLite features; its manual says encryption is experimental and not ready for critical data. |

### Migration library comparison

| Candidate | Fit and trade-offs |
| --- | --- |
| `rusqlite_migration` 2.6.0 | Preferred candidate. Apache-2.0, Rust 1.95, and rusqlite 0.40 support align with the workspace and the selected rusqlite line. Its normal dependencies are `log` and `rusqlite`; `include_dir` is optional. Migrations use SQLite `user_version` and run atomically. The release history shows regular updates, most recently 2026-05-28. |
| `refinery` 0.10.0 | Viable alternative. MIT, Rust 1.92, and rusqlite 0.40 support. It offers multiple database backends, migration macros, and a separate CLI. Those broader features add crate and macro dependencies that the SQLite-only desktop cache does not currently need. |

## Consequences

- Persistence owns SQL schemas, migrations, encryption setup, and transactions. It commits Graph changes and the matching cursor together.
- SQLCipher is a deliberate native-code dependency in an otherwise Rust-first application because the compared pure-Rust database options do not meet the current relational and encryption requirements.
- A proposed integration must build and run on Windows x64, create an encrypted database, write and reopen it, reject a wrong key, and apply migrations through the selected library set.
- Before accepting the ADR, verify key creation and access through the Windows credential store, missing-key behavior, cache deletion on account removal, and what happens to local-only pending data if the key is lost.
- Do not put mailbox content or OAuth tokens in test fixtures, logs, or the credential-store test evidence.

## Sources

- [`rusqlite` 0.40.2 documentation](https://docs.rs/crate/rusqlite/0.40.2)
- [`rusqlite` 0.40.2 SQLCipher build and license notes](https://docs.rs/crate/rusqlite/0.40.2/source/README.md)
- [`rusqlite_migration` 2.6.0 documentation and dependency list](https://docs.rs/crate/rusqlite_migration/2.6.0)
- [`rusqlite_migration` 2.6.0 manifest](https://docs.rs/crate/rusqlite_migration/2.6.0/source/Cargo.toml.orig)
- [`rusqlite_migration` release history](https://github.com/cljoly/rusqlite_migration/releases)
- [`refinery` release notes](https://github.com/rust-db/refinery/releases)
- [`refinery` 0.10.0 manifest](https://docs.rs/crate/refinery/0.10.0/source/Cargo.toml)
- [`SQLx` SQLite driver documentation](https://docs.rs/sqlx/0.9.0/sqlx/sqlite/)
- [`redb` documentation](https://docs.rs/redb/4.4.0/redb/)
- [Turso manual](https://github.com/tursodatabase/turso/blob/main/docs/manual.md)
- [`rusqlite` Windows SQLCipher report](https://github.com/rusqlite/rusqlite/issues/966)
