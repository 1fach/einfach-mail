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

GPUI Kit documents these requirements for desktop development:

- macOS 15 or later with Xcode Command Line Tools.
- Windows 10 or later with Visual Studio 2022 Build Tools or Community, the Desktop development with C++ workload, a Windows SDK, and CMake. Use the MSVC Rust toolchain.
- Ubuntu 24.04 with the packages below. Other Linux distributions need equivalent development packages.

```sh
sudo apt update
sudo apt install -y gcc g++ clang pkg-config libfontconfig-dev libwayland-dev \
  libxkbcommon-x11-dev libx11-xcb-dev libssl-dev libzstd-dev \
  vulkan-validationlayers libvulkan1
```

GPUI Kit's full development setup also lists WebKitGTK and ALSA. The current app uses neither the separate `gpui-webview` crate nor GPUI Kit's optional speech feature, so they are not in the shell's Linux package list. If a later renderer decision adds `gpui-webview`, install WebKitGTK 4.1 and GTK 3 development packages. Linux embedding requires X11 or XWayland.

On Fedora 44, install the corresponding development packages with:

```sh
sudo dnf install gcc gcc-c++ clang pkgconf-pkg-config \
  fontconfig-devel freetype-devel wayland-devel wayland-protocols-devel \
  libxkbcommon-x11-devel libX11-devel libxcb-devel openssl-devel \
  libzstd-devel vulkan-loader-devel vulkan-validation-layers
```

The package list follows GPUI Kit's Linux build requirements. It omits GTK and WebKitGTK because the desktop app does not include the optional email WebView.

## Initialize Git and the main branch

If `git status` says that this directory is not a repository, initialize Git on `main`:

```sh
git init -b main
```

If Git is already initialized, has no commits, and the current branch is `master`, rename it before the first commit:

```sh
git branch -m main
```

Create the first commit:

```sh
git add .
git commit -m "chore: establish project scaffold"
```

Create a remote repository, then connect it:

```sh
git remote add origin <repository-url>
git push --set-upstream origin main
```

If the hosting plan supports branch protection, protect `main` by requiring pull requests and these CI checks before merging:

- `Rust checks (ubuntu-latest)`
- `Rust checks (windows-latest)`
- `Rust checks (macos-latest)`
- `Rust MSRV check 1.95.0 (ubuntu-latest)`
- `Rust MSRV check 1.95.0 (windows-latest)`
- `Rust MSRV check 1.95.0 (macos-latest)`
- `Workspace dependency policy`
- `RustSec audit`

A solo project can omit reviewer approval while still requiring CI. Do not enable auto-merge until the first CI run has succeeded. If server-side protection is unavailable, use pull requests and verify all eight checks manually before merging; document that the gate is not enforced by the host.

## Start the desktop shell

From the repository root, run:

```sh
cargo run --locked --package einfach-mail-desktop --bin einfach-mail
```

The window is a native GPUI shell. It does not sign in or access mailbox data yet.

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

`cargo audit --deny yanked` reads `Cargo.lock`, fails when a locked crate is yanked, and does not update the lockfile. Do not suppress advisories globally; resolve them or record a narrow, reviewed exception.

The [RustSec audit workflow](../../.github/workflows/rustsec-audit.yml) runs on pull requests, pushes to `main`, and weekly.

Install the pinned workspace dependency policy tools:

```sh
cargo install cargo-depgraph-check --version 0.1.0 --locked
cargo install cargo-deny --version 0.20.2 --locked
cargo depgraph-check check
cargo deny check bans
```

The strict allowlist in `depgraph-rules.toml` includes every workspace crate and its permitted direct internal dependencies, including dev-dependencies. Update it when adding a workspace crate or changing a crate boundary. The checker validates direct workspace edges; it does not inspect module-level boundaries inside a crate. `cargo deny check bans` enforces the explicit Tauri runtime and WebView crate bans in `deny.toml`. Run `bash scripts/check-no-tauri-target-dependencies.sh` to check the active dependency graph for each supported target. The target-disjoint `tauri-winrt-notification` lockfile entry is not part of an app build.

## Run project checks

Run these commands from the repository root:

```sh
cargo depgraph-check check
cargo deny check bans
cargo +1.95.0 check --workspace --all-targets --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo doc --workspace --no-deps --locked
cargo audit --deny yanked
cargo test --workspace --locked
```

Use `cargo fmt --all` to format Rust files. Fix Clippy findings instead of suppressing them. Add an `#[expect(..., reason = "...")]` only for a narrow lint that is demonstrably wrong.
