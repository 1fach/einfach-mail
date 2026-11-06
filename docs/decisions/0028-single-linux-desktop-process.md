# ADR 0028: Allow one Linux desktop process per user

- Status: Accepted
- Date: 2026-11-04
- Owners: Einfach Mail maintainers

## Context

Mail and calendar workflows coordinate through an in-memory reservation registry. Separate desktop processes would have separate registries and SQLCipher clients for the same account files. The cache's directory lock protects removal and send staging, but ordinary reads and writes do not hold that lock for their full database lifetime.

The Linux desktop opens one window and has no activation-forwarding protocol. Multiple processes would not provide a useful second-window experience.

## Decision

- Acquire an exclusive lock on `XDG_DATA_HOME/einfach-mail/instance.lock` before GPUI initialization. Keep the file open for the application's lifetime.
- Use `xdg` 3.0.0 to create the lock-file path. Open the file with mode `0600` and use `std::fs::File::try_lock` for the non-blocking process lock.
- Keep the lock file after shutdown. Removing a locked file could let another process lock a new file at the same path.
- If another process holds the lock, return an `AlreadyExists` startup error before creating the GPUI application, account workflow, or cache client.
- Do not add IPC or forward activation to the running window.
- Keep the child-process lock test synthetic. Do not launch a desktop window.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Use `std::fs::File::try_lock` at startup | Uses a maintained standard-library API that supports process locks. It adds no lock dependency and runs before the async UI starts. | A second launch exits instead of focusing the existing window. The lock only coordinates processes that use this file. |
| Add cross-process locks to every cache operation | Could allow multiple desktop processes to use one cache. | Requires a cache lifecycle protocol for open handles, removal, and stale clients. A per-operation lock does not invalidate an already-open SQLite connection. |
| Add a single-instance crate | May provide a reusable wrapper. | The standard library already provides the required exclusive lock. The app does not need activation forwarding. |

## Consequences

- Supported desktop workflows share one process and one in-memory reservation registry.
- A second launch reports that Einfach Mail is already running and exits.
- The operating system releases the lock when the process exits or crashes.
- The lock is advisory. Software that ignores it can still access the same files.
- The lock file contains no account data or secret. Its permissions are restricted to the current user.
- A synthetic child-process test verifies that a second process is rejected and that startup succeeds after the lock owner exits.

## Sources

- [Rust `File::try_lock`](https://doc.rust-lang.org/1.95.0/std/fs/struct.File.html#method.try_lock)
- [`xdg::BaseDirectories::place_data_file`](https://docs.rs/xdg/3.0.0/xdg/struct.BaseDirectories.html#method.place_data_file)
- [`xdg` 3.0.0 package metadata](https://docs.rs/crate/xdg/3.0.0)
- [GitHub issue #43: Serialize account removal with active mail operations](https://github.com/1fach/einfach-mail/issues/43)
- [ADR 0027: Serialize mail account removal with mail operations](0027-serialize-mail-account-removal-with-mail-operations.md)
