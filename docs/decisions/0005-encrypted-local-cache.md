# ADR 0005: Use SQLCipher-backed SQLite for the local cache

- Status: Proposed; Windows and key-lifecycle proof required
- Date: 2026-10-31
- Owners: Project maintainer

## Context

ADR 0004 requires durable offline mail and calendar data, with Graph changes and their sync cursor committed atomically. That cache needs relational queries for folders, message lists, calendar ranges, and pending operations. The product also requires an at-rest encryption decision before real mailbox data is stored.

The app is a native Rust desktop application. A pure-Rust UI and application stack is preferred, but the database choice must also provide mature relational behavior and a maintained encryption implementation. The first support target is Windows.

## Decision

Prefer SQLCipher-backed SQLite behind the planned `crates/persistence/` boundary. The application and domain crates will not depend on rusqlite or SQLCipher types.

The current candidate set is `async-sqlite` 0.6.0 with `rusqlite_migration` 2.6.0. Configure `async-sqlite` with default features disabled and enable `bundled-sqlcipher-vendored-openssl`. Its default features include plain bundled SQLite and a WebAssembly SQLite backend, neither of which this desktop app needs. Its SQLCipher feature forwards to rusqlite. The wrapper uses a background worker and bounded queues, and its maintainers test it with both `smol` and Tokio. Use its single-connection `Client` unless a measured workload needs a pool.

The reviewed migration and executor crates depend on rusqlite 0.40.x, so Cargo resolves `rusqlite` 0.40.2. `rusqlite` 0.41.1 is newer, but the current `async-sqlite` and `rusqlite_migration` releases do not accept that line. Revisit this constraint when either library supports rusqlite 0.41.

`rusqlite_migration` uses SQLite's `user_version` and applies migrations atomically. It has a smaller SQLite-specific dependency surface than Refinery 0.10.0, which supports rusqlite 0.40.x and Rust 1.92 but also provides other database backends, macros, and a CLI. Prefer `rusqlite_migration` for the SQLite-only cache. The exact candidate dependencies now appear as development dependencies in `tools/storage-proof/`; the package has no runtime code or application API. RustSec review and Windows x64 proof remain required before adding these dependencies to production crates.

`async-rusqlite` 0.6.0 is an alternative wrapper for rusqlite 0.41. It declares no MSRV, depends on `asyncified` for its channel implementation, and its README describes those channels as naive. Its README recommends `tokio-rusqlite` when executor independence is not needed. Do not use it for the current candidate set.

Use `keyring` 4.2.0 with its default `v1` API as the cross-platform credential-store candidate. On Windows, that API selects `windows-native-keyring-store` 1.1.0 and Windows Credential Manager. Do not enable `keyring`'s `cli` feature. The key lifecycle, missing-key recovery, cache backup, and account-removal deletion behavior remain open, so this ADR is not accepted and no real mailbox data may be stored yet.

The proof uses an OS-generated 32-byte key, stores it as binary credential data, and zeroizes Rust-owned copies. The backup destination receives the same SQLCipher key before SQLite's online backup API writes to it. The key is not included in the backup, so the proof only supports reopening a backup while the matching credential is available. Portable backup recovery remains an open product decision.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| SQLCipher-backed SQLite through rusqlite | Relational queries, transactions, bundled encryption, mature SQLite ecosystem; Rust API stays behind the persistence adapter | SQLCipher and OpenSSL add native C build and licensing inputs. The vendored OpenSSL build needs a C compiler, Perl, and make. Windows also needs MSVC and NASM. A community Windows x64 report uses static OpenSSL from vcpkg and does not prove the vendored build. Windows encrypted read/write and credential-store behavior need project proof. |
| SQLx with SQLite | Async query API, migrations, supports SQLite and server databases | The documented SQLite feature set does not offer a supported SQLCipher configuration. The SQLite driver also uses `libsqlite3-sys`, which must be coordinated with any rusqlite dependency. |
| redb | Pure Rust, ACID transactions, concurrent readers | Key-value model increases query/index work for mail and calendar; the project docs do not provide database encryption. |
| Turso | In-process relational engine implemented in Rust and compatible with much of SQLite | The project documents beta status and missing SQLite features; its manual says encryption is experimental and not ready for critical data. |

### Migration library comparison

| Candidate | Fit and trade-offs |
| --- | --- |
| `rusqlite_migration` 2.6.0 | Preferred candidate. Apache-2.0, Rust 1.95, and rusqlite 0.40 support align with the workspace and the selected rusqlite line. Its normal dependencies are `log` and `rusqlite`; `include_dir` is optional. Migrations use SQLite `user_version` and run atomically. The release history shows regular updates, most recently 2026-05-28. |
| `refinery` 0.10.0 | Viable alternative. Its latest release is dated 2026-10-02. It is MIT-licensed, declares Rust 1.92, and supports rusqlite 0.40.x. Its default feature is `toml`; SQLite support is opt-in through `rusqlite` or `rusqlite-bundled`. The crate also depends on `refinery-macros`; its CLI is a separate package. The SQLite-only cache does not need those broader features. |

### Executor wrapper comparison

| Candidate | Fit and trade-offs |
| --- | --- |
| `async-sqlite` 0.6.0 | Preferred candidate. Its latest release is dated 2026-05-27. MIT, Rust 1.95, and rusqlite 0.40 support match the workspace. It uses `crossbeam-channel` and `futures`, exports SQLCipher feature flags, and documents tests with `smol` and Tokio. Its default features also enable plain bundled SQLite, `cache`, and `ffi-sqlite-wasm-rs`; disable defaults and enable only the reviewed SQLCipher build feature. Its `Client` provides one background connection with a bounded queue. |
| `async-rusqlite` 0.6.0 | MIT and executor-agnostic, with rusqlite 0.41 support. It does not declare an MSRV or expose SQLCipher feature flags. Its README says its `asyncified` channel implementation is naive and recommends `tokio-rusqlite` when executor independence is unnecessary. |

## Consequences

- Persistence owns SQL schemas, migrations, encryption setup, and transactions. It commits Graph changes and the matching cursor together.
- SQLCipher is a deliberate native-code dependency in an otherwise Rust-first application because the compared pure-Rust database options do not meet the current relational and encryption requirements.
- A proposed integration must build and run on Windows x64, create an encrypted database, write and reopen it, reject a wrong key, and apply migrations through the selected library set.
- Before accepting the ADR, verify key creation and access through the Windows credential store, missing-key behavior, cache deletion on account removal, and what happens to local-only pending data if the key is lost.
- The test-only backup proof applies the SQLCipher key to both database connections and verifies that the backup is encrypted and reopens with the original key. It does not export or package the key.
- Do not put mailbox content or OAuth tokens in test fixtures, logs, or the credential-store test evidence.

## Sources

- [`rusqlite` 0.40.2 documentation](https://docs.rs/crate/rusqlite/0.40.2)
- [`rusqlite` 0.41.1 documentation](https://docs.rs/crate/rusqlite/0.41.1)
- [`rusqlite` 0.40.2 SQLCipher build and license notes](https://docs.rs/crate/rusqlite/0.40.2/source/README.md)
- [`rusqlite_migration` 2.6.0 documentation and dependency list](https://docs.rs/crate/rusqlite_migration/2.6.0)
- [`rusqlite_migration` 2.6.0 manifest](https://docs.rs/crate/rusqlite_migration/2.6.0/source/Cargo.toml.orig)
- [`rusqlite_migration` release history](https://github.com/cljoly/rusqlite_migration/releases)
- [`refinery` release notes](https://github.com/rust-db/refinery/releases)
- [`refinery` 0.10.0 manifest](https://docs.rs/crate/refinery/0.10.0/source/Cargo.toml)
- [`async-sqlite` 0.6.0 documentation and runtime support](https://docs.rs/crate/async-sqlite/0.6.0)
- [`async-sqlite` 0.6.0 manifest and features](https://docs.rs/crate/async-sqlite/0.6.0/source/Cargo.toml.orig)
- [`async-rusqlite` 0.6.0 documentation](https://docs.rs/crate/async-rusqlite/0.6.0)
- [`async-rusqlite` 0.6.0 manifest](https://docs.rs/crate/async-rusqlite/0.6.0/source/Cargo.toml.orig)
- [`keyring` 4.2.0 documentation and feature list](https://docs.rs/crate/keyring/4.2.0)
- [`keyring` release notes](https://github.com/open-source-cooperative/keyring-rs/releases)
- [`windows-native-keyring-store` 1.1.0 manifest](https://docs.rs/crate/windows-native-keyring-store/1.1.0/source/Cargo.toml.orig)
- [SQLCipher key and raw-key documentation](https://github.com/sqlcipher/sqlcipher#encrypting-a-database)
- [`rusqlite` 0.40.2 online backup API](https://docs.rs/rusqlite/0.40.2/rusqlite/backup/struct.Backup.html)
- [`getrandom` 0.4.3](https://docs.rs/getrandom/0.4.3/getrandom/fn.fill.html)
- [`zeroize` 1.9.1](https://docs.rs/zeroize/1.9.1/zeroize/struct.Zeroizing.html)
- [`smol` 2.0.2](https://docs.rs/smol/2.0.2/smol/fn.block_on.html)
- [`hex` 0.4.3](https://docs.rs/hex/0.4.3/hex/fn.encode.html)
- [`tempfile` 3.27.0](https://docs.rs/tempfile/3.27.0/tempfile/struct.TempDir.html)
- [RustSec advisories for rusqlite](https://rustsec.org/packages/rusqlite.html)
- [OpenSSL crate build requirements](https://docs.rs/openssl/latest/openssl/#vendored)
- [OpenSSL Windows build requirements](https://github.com/openssl/openssl/blob/master/NOTES-WINDOWS.md)
- [`SQLx` SQLite driver documentation](https://docs.rs/sqlx/0.9.0/sqlx/sqlite/)
- [`redb` documentation](https://docs.rs/redb/4.4.0/redb/)
- [Turso manual](https://github.com/tursodatabase/turso/blob/main/docs/manual.md)
- [`rusqlite` Windows SQLCipher report](https://github.com/rusqlite/rusqlite/issues/966)
