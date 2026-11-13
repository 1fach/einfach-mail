# Development setup

## Install Rust

Install Rust with `rustup`, then install the components declared in `rust-toolchain.toml`:

```sh
rustup toolchain install 1.95.0
rustup toolchain install stable --component rustfmt --component clippy
rustup default stable
```

If `cargo` or `rustc` is not found but `~/.cargo/bin` exists, add that directory to the current shell's `PATH`:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
```

GPUI Kit's published docs state Rust 1.92 or newer, but the locked GPUI dependency graph uses APIs stable in Rust 1.95. Einfach Mail therefore requires Rust 1.95 or newer; see the [MSRV evidence](../research/gpui-kit.md#minimum-rust-version).

## Install native build tools

Linux is the only current development target. Add Windows and macOS setup instructions when those support milestones begin.

GPUI Kit documents these requirements for Linux desktop development:

- Ubuntu 24.04 with the packages below. Other Linux distributions need equivalent development packages.

```sh
sudo apt update
sudo apt install -y gcc g++ clang make perl libtext-template-perl pkg-config libfontconfig-dev libwayland-dev \
  libxkbcommon-x11-dev libx11-xcb-dev libssl-dev libzstd-dev \
  vulkan-validationlayers libvulkan1
```

GPUI Kit's full development setup also lists WebKitGTK and ALSA. The current app uses neither the separate `gpui-webview` crate nor GPUI Kit's optional speech feature, so they are not in the shell's Linux package list. If a later renderer decision adds `gpui-webview`, install WebKitGTK 4.1 and GTK 3 development packages. Linux embedding requires X11 or XWayland.

On Fedora 44, install the corresponding development packages with:

```sh
sudo dnf install gcc gcc-c++ clang pkgconf-pkg-config \
  fontconfig-devel freetype-devel wayland-devel wayland-protocols-devel \
  libxkbcommon-x11-devel libX11-devel libxcb-devel openssl-devel \
  libzstd-devel vulkan-loader-devel vulkan-validation-layers make \
  perl-FindBin perl-Text-Template
```

The persistence crate builds SQLCipher with vendored OpenSSL. Fedora packages Perl's `FindBin` and `Text::Template` modules separately, so install both packages before building or testing the workspace.

The package list follows GPUI Kit's Linux build requirements. It omits GTK and WebKitGTK because the desktop app does not include the optional email WebView.

## Use the configured Git repository

This repository already has a `main` branch and an `origin` remote. Do not initialize Git or create a first commit in this checkout. Before editing, check the current branch and preserve existing work:

```sh
git status --short --branch
```

The private repository plan does not support required branch protection. Run the Linux project gate locally and review pull request checks before merging. The repository defines these GitHub Actions checks: `Rust checks (Linux)`, `Rust MSRV check 1.95.0 (Linux)`, `Workspace dependency policy`, and `RustSec audit`.

## Run the Linux desktop app

To inspect the native workspace without a mail account, run the developer-only UI preview:

```sh
cargo run --locked --package einfach-mail-desktop --features ui-preview --bin einfach-mail
```

The preview opens the same Mail, Calendar, Accounts, and Settings views with synthetic `example.test` data. Its providers and caches stay in memory; it does not use credentials, Secret Service, SQLite, or the normal user appearance file. Theme changes go to a temporary directory that is removed when the app exits. The `ui-preview` feature is rejected in release builds. This preview checks the native Linux UI only; it does not verify Graph, EWS, or live account behavior.

To enable the account view's sign-in action in the regular app, set `EINFACH_MAIL_ENTRA_CLIENT_ID` to the client ID of a public Microsoft Entra app registration. Set `EINFACH_MAIL_ENTRA_CALLBACK_PORT` to the nonzero port registered for `http://127.0.0.1:<port>/callback`. The app does not use a client secret. Initial sign-in requests delegated `User.Read`, `Mail.Read`, `Calendars.Read`, and `offline_access`. It reads the signed-in profile through Graph `/me` and stores the refresh token in Linux Secret Service. The app requests `Mail.ReadWrite`, `Calendars.ReadWrite`, or `Mail.Send` only in a separate same-account permission review after Microsoft denies the related action. Each permission gives broader access than the app's current actions. The app explains the full Microsoft capability before opening the system browser. See the [Graph permission table](../research/microsoft-graph.md#permissions) and [ADR 0045](../decisions/0045-incremental-graph-permissions.md).

Run the app from the repository root:

```sh
export EINFACH_MAIL_ENTRA_CLIENT_ID='<public-app-client-guid>'
export EINFACH_MAIL_ENTRA_CALLBACK_PORT='<registered-loopback-port>'
cargo run --locked --package einfach-mail-desktop --bin einfach-mail
```

If the client ID or callback port is missing or invalid, the account view explains how to configure sign-in and keeps account actions unavailable. The callback binds only to `127.0.0.1` and uses the registered port exactly. Microsoft documents that only `localhost` ignores port differences. RFC 8252 requires arbitrary ports for loopback IP redirects. This conflict needs tenant confirmation; see [ADR 0031](../decisions/0031-registered-loopback-port-for-entra.md).

The mail and calendar views load cached data and start synchronization. Synthetic provider, workflow, persistence, and headless UI tests cover these paths. Live Microsoft Graph behavior remains unverified because no dedicated Microsoft 365 test tenant or public app registration is available. Do not sign in with a personal or production account. Issue #3 tracks live callback, browser, MFA, Conditional Access, refresh, consent-revocation, and sign-out evidence.

## Resolve and review dependencies

Run the locked check from the repository root:

```sh
cargo check --workspace --locked
```

When intentionally changing dependencies, run `cargo check --workspace` without `--locked` to update the lockfile. Review the `Cargo.lock` diff and dependency tree (`cargo tree --workspace --duplicates --locked`), then commit the lockfile. Never hand-edit it.

Install the pinned RustSec audit tool and check the lockfile against the current advisory database:

```sh
cargo install cargo-audit --version 0.22.2 --locked --no-default-features
cargo audit --deny yanked
```

`cargo audit` reads `Cargo.lock`, fails when a locked crate is yanked, and does not update the lockfile. By default, it uses `~/.cargo/advisory-db` and may fetch updates. To audit offline with an existing database, set `EINFACH_RUSTSEC_DB` when you run `scripts/check-linux-local.sh`; the script skips fetching for that run. Refresh the database while online before relying on offline results. Do not suppress advisories globally; resolve them or record a narrow, reviewed exception.

The [RustSec audit workflow](../../.github/workflows/rustsec-audit.yml) runs on pull requests, pushes to `main`, and weekly.

Install the pinned workspace dependency policy tools:

```sh
cargo install cargo-depgraph-check --version 0.1.0 --locked
cargo install cargo-deny --version 0.20.2 --locked
cargo depgraph-check check
cargo deny check bans
```

The strict allowlist in `depgraph-rules.toml` includes every workspace crate and its permitted direct internal dependencies, including dev-dependencies. Update it when adding a workspace crate or changing a crate boundary. The checker validates direct workspace edges; it does not inspect module-level boundaries inside a crate. `cargo deny check bans` enforces the exact `wry` and `gpui-webview` bans in `deny.toml`. These bans keep the optional WebView outside the application until the body-only renderer passes the security review in ADR 0007.

## Run project checks

Run the full Linux gate from the repository root before updating a branch:

```sh
bash scripts/check-linux-local.sh
```

The script runs the Rust 1.95 check, dependency-boundary and ban checks, the active Linux target graph, formatting, Clippy, documentation, RustSec, and workspace tests. If Perl lacks `FindBin.pm` but `pkg-config` finds system OpenSSL, it reports that it uses system OpenSSL for the local run. The workspace test command skips tests marked `ignore`.

Run the persistence tests with:

```sh
cargo test --locked --package einfach-mail-persistence
```

The default tests include a child-process check that verifies production cache opening leaves new and existing cache files unchanged when Secret Service is unavailable. The ignored lifecycle test uses the production cache API to create, encrypt, reopen, preserve through key loss, and remove a synthetic cache. It requires an unlocked Linux Secret Service and can be run explicitly with:

```sh
cargo test --locked --package einfach-mail-persistence --lib platform::linux::security::tests::production::production_secret_service_cache_lifecycle -- --ignored --exact
```

The test uses a random synthetic account ID, isolated XDG storage in a child process, and synthetic folder data. The earlier Windows Credential Manager proof in PR #15 is platform-specific evidence; Windows support remains deferred.

Use `cargo fmt --all` to format Rust files. Fix Clippy findings instead of suppressing them. Add an `#[expect(..., reason = "...")]` only for a narrow lint that is demonstrably wrong.
