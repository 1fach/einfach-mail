# ADR 0005: Use SQLCipher-backed SQLite for the local cache

- Status: Accepted for Linux; production cache adapter and key-lifecycle tests implemented
- Date: 2026-10-31
- Owners: Project maintainer

## Context

ADR 0004 requires durable offline mail and calendar data, with Graph changes and their sync cursor committed atomically. That cache needs relational queries for folders, message lists, calendar ranges, and pending operations. Mailbox data requires encryption at rest and account-scoped removal.

The app is a native Rust desktop application. A pure-Rust UI and application stack is preferred, but the database choice must also provide mature relational behavior and a maintained encryption implementation. Linux is the only current development target. The original Linux Secret Service key-lifecycle proof passed on Fedora on 2026-10-31. Production persistence now tests encryption, transactions, checkpoint recovery, offline body reads, cache removal, and the Secret Service-backed cache-key lifecycle. The key-lifecycle test passed in an isolated GNOME Keyring D-Bus session on 2026-11-08; it does not verify every Secret Service implementation or a real Graph mailbox. See [project status](../project/status.md).

## Decision

Use SQLCipher-backed SQLite behind `crates/persistence/`. The application and domain crates do not depend on rusqlite or SQLCipher types.

The production cache uses `async-sqlite` 0.6.0 with default features disabled and `bundled-sqlcipher-vendored-openssl`, plus `rusqlite_migration` 2.6.0. The wrapper's background worker and bounded queue keep SQLite work off the GPUI executor. Its default features include plain bundled SQLite and a WebAssembly SQLite backend, neither of which this desktop app needs. Use its single-connection `Client` unless a measured workload needs a pool. Resolve Linux user data paths with `xdg` 3.0.0 and keep cache files under the XDG data home. The `directories` 6.0.0 alternative has a broader project-directory API, but its GitHub upstream is archived and moved to Codeberg; `xdg` is the narrower fit for the current Linux target. Recheck maintenance before adopting another platform.

The reviewed migration and executor crates depend on rusqlite 0.40.x, so Cargo resolves `rusqlite` 0.40.2. `rusqlite` 0.41.1 is newer, but the current `async-sqlite` and `rusqlite_migration` releases do not accept that line. Revisit this constraint when either library supports rusqlite 0.41.

`rusqlite_migration` uses SQLite's `user_version` and applies migrations atomically. It has a smaller SQLite-specific dependency surface than Refinery 0.10.0, which supports rusqlite 0.40.x and Rust 1.92 but also provides other database backends, macros, and a CLI. Use `rusqlite_migration` for the SQLite-only cache. Keep SQLCipher setup, schema migration, and key lifecycle in `crates/persistence`; tests exercise the production adapter rather than a second implementation.

`async-rusqlite` 0.6.0 is an alternative wrapper for rusqlite 0.41. It declares no MSRV, depends on `asyncified` for its channel implementation, and its README describes those channels as naive. Its README recommends `tokio-rusqlite` when executor independence is not needed. Do not use it for the current candidate set.

Use `keyring` 4.2.0 with its `v1` API for Linux credential storage. This selects the zbus Secret Service backend. Do not enable `keyring`'s `cli` feature. The earlier Windows x64 proof in [PR #15](https://github.com/1fach/einfach-mail/pull/15) exercised Windows Credential Manager, but Windows support remains deferred. Recheck the credential-store choice and run its platform proof when Windows or macOS support begins.

The original Linux test-only proof passed on 2026-10-31 in a Fedora desktop session with GNOME Keyring. It reopened an encrypted database, rejected missing or wrong keys, and removed temporary account data. Its backup-specific tests were removed on 2026-11-07 because production has no backup workflow, and the standalone code duplicated production key and encryption logic. [PR #21](https://github.com/1fach/einfach-mail/pull/21) merged the original proof and closed issue #20.

The production-path tests now use `SqliteAccountCache::open_for_account` and `remove_for_account`. The ignored lifecycle test passed on 2026-11-08 in a temporary GNOME Keyring D-Bus session. It proves cache creation, encrypted reopen, wrong and malformed key rejection, missing-key byte preservation, and idempotent removal of the cache and key. The default child-process test uses an unavailable D-Bus address and proves that new cache files are not created and existing encrypted files are unchanged. A unit test injects `NoStorageAccess` and verifies that this credential error is not treated as a missing key. Tests use random synthetic account IDs and folder data; they do not establish behavior for every Secret Service implementation.

The production adapter uses an OS-generated 32-byte key, stores it as binary credential data, and zeroizes Rust-owned copies. It recognizes `mail.sqlite3` and its `-wal` and `-shm` sidecars when it checks for existing encrypted data. If the credential is missing while one of those files exists, production must not create a replacement key or replay pending local actions. Account removal deletes those recognized files and the SQLCipher key. Production does not create, restore, or delete backup files. Portable backup recovery and backup-file removal remain open product decisions.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| SQLCipher-backed SQLite through rusqlite | Relational queries, transactions, bundled encryption, mature SQLite ecosystem; Rust API stays behind the persistence adapter | SQLCipher and OpenSSL add native C build and licensing inputs. The vendored OpenSSL build needs a C compiler, Perl, `Text::Template`, and make. Windows also needs MSVC and NASM; the Windows proof in PR #15 installed these tools and passed on x64. Current CI is Linux-only. |
| SQLx with SQLite | Async query API, migrations, supports SQLite and server databases | The documented SQLite feature set does not offer a supported SQLCipher configuration. The SQLite driver also uses `libsqlite3-sys`, which must be coordinated with any rusqlite dependency. |
| redb | Pure Rust, ACID transactions, concurrent readers | Key-value model increases query/index work for mail and calendar; the project docs do not provide database encryption. |
| Turso | In-process relational engine implemented in Rust and compatible with much of SQLite | The project documents beta status and missing SQLite features; its manual says encryption is experimental and not ready for critical data. |

### Linux credential-store comparison

| Candidate | Fit and trade-offs |
| --- | --- |
| `keyring` 4.2.0 | Preferred candidate. Its `v1` API provides synchronous binary secret operations and selects the zbus Secret Service backend on Linux. The same API has native store adapters for Windows and macOS when those support milestones begin. Keep the `cli` feature disabled. |
| `oo7` 0.6.0 | Maintained Secret Service client with D-Bus and encrypted file backends, including portal integration for sandboxed applications. It is async-only and documents Tokio, async-std, or GLib runtime use. The current GPUI application is not sandboxed and uses `smol`; its file/portal support and runtime choices do not fit the present boundary as closely as `keyring`. |

Production persistence tests use only `keyring`'s Secret Service path. The default test run verifies that an unavailable service neither creates new cache files nor changes existing encrypted files. The test that creates, retrieves, and removes synthetic credentials requires an unlocked Secret Service and remains opt-in.

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

- Persistence owns SQL schemas, migrations, encryption setup, and transactions. The production Linux adapter commits Graph page changes and matching cursors together.
- SQLCipher is a deliberate native-code dependency in an otherwise Rust-first application because the compared pure-Rust database options do not meet the current relational and encryption requirements.
- The earlier Windows test-only proof in PR #15 built vendored SQLCipher and OpenSSL on Windows x64. It exercised Windows Credential Manager, migrations, encrypted reads and writes, wrong-key rejection, backup reopening, missing-key behavior, and account removal.
- The production-path cache-key lifecycle test passed on 2026-11-08 in an isolated GNOME Keyring D-Bus session. A synthetic `NoStorageAccess` error covers the credential-store failure classification; behavior across other Secret Service implementations remains unverified.
- Losing a cache key leaves the encrypted files intact but makes their contents inaccessible. Production reports that state and does not generate a replacement key or replay pending local actions.
- Production tests do not test backup creation or recovery. The application does not create, restore, or delete backup files. Portable backup recovery and backup-file removal remain open product decisions.
- Synthetic persistence tests cover account-scoped folders, fixed cutoffs, page/cursor atomicity, restart resume, opened-body caching, and 5,001-message pagination. They do not establish full Graph behavior; issue #3 and the tenant-only issue #8 acceptance criteria remain open.
- Signing out a connected account removes its encrypted database, SQLite sidecars, and SQLCipher key before deleting the saved Microsoft sign-in. If cache cleanup fails, the desktop keeps the account connected so the user can retry.
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
- [`keyring` 4.2.0 v1 API and platform stores](https://docs.rs/keyring/4.2.0/keyring/v1/index.html)
- [`keyring` release notes](https://github.com/open-source-cooperative/keyring-rs/releases)
- [`oo7` 0.6.0 documentation](https://docs.rs/oo7/0.6.0/oo7/)
- [`oo7` repository and release history](https://github.com/linux-credentials/oo7/releases)
- [`windows-native-keyring-store` 1.1.0 manifest](https://docs.rs/crate/windows-native-keyring-store/1.1.0/source/Cargo.toml.orig)
- [SQLCipher key and raw-key documentation](https://github.com/sqlcipher/sqlcipher#encrypting-a-database)
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
