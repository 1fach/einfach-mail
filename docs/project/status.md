# Project status

Last updated: 2026-10-30

## Current milestone

Milestone 0, repository foundation. The workspace now has a native GPUI Kit app shell. Account sign-in, mail, calendar, and synchronization are not implemented.

## Verified state

- Rust and Cargo 1.99.1 are installed under `~/.cargo/bin`, but that directory is missing from this execution environment's `PATH`.
- `cargo fmt --all -- --check` passes, and `cargo metadata --no-deps` recognizes the three workspace crates.
- `Cargo.lock` resolves `gpui-kit` 0.8.1 and GPUI snapshot 0.3.10. `cargo tree` shows the desktop has only GPUI Kit as a direct external dependency; its default component and asset layers are used by the current shell.
- `cargo check --workspace --locked --offline` reaches native compilation but stops because Fedora development packages such as Fontconfig and a C++ compiler are missing. The packages are documented in [development setup](setup.md); this environment's `sudo` requires a password, so they were not installed.
- Git is initialized on `main`; the project scaffold, resolved `Cargo.lock`, and pinned GPUI Kit agent skills are captured by the initial baseline commit.
- The workspace contains pinned GPUI Kit coding and design skills from `longbridge/gpui-kit`; their source and hashes are recorded in `skills-lock.json`.
- The old desktop shell and browser assets have been removed. The workspace and source now point to GPUI Kit.
- The GPUI Kit v0.8.1 API, platform requirements, and optional WebView limits were checked against its current documentation on 2026-10-30.
- The dependency review records candidates for Graph, authentication, credential storage, SQLite, MIME, and HTML sanitization. No other direct dependency has been added.

## Active risks

- GPUI Kit is pre-1.0, and this shell has not been compiled or run on any platform.
- The optional message-body HTML rendering approach is not selected. The WebView crate has platform limits and is experimental.
- The OAuth implementation and secure token storage need a proof of concept before real account access.
- The local database engine and at-rest encryption approach are not selected.
- Exchange Server on premises is not supported by the first provider plan.

## Next actions

1. Add `~/.cargo/bin` to the shell `PATH` and install the Fedora build packages from [development setup](setup.md), then rerun the locked offline build.
2. Build and launch the GPUI shell on Fedora and Windows.
3. Compare maintained Rust authentication libraries and platform brokers, then run the system-browser PKCE spike.
4. Choose local storage, schema migration, encryption, and key recovery before storing real mail or calendar data.
5. Choose a maintained HTML parser and sanitizer and a message-body rendering approach before implementing rich email.
6. Create a first vertical slice that signs in and reads the user's account identity with delegated permissions.

## Update rule

Update this file when a milestone, blocker, or next action changes. Keep task-by-task history in issues and pull requests.
