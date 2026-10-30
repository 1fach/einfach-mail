# Development setup

## Install Rust

Install Rust with `rustup`, then install the components declared in `rust-toolchain.toml`:

```sh
rustup toolchain install stable --component rustfmt --component clippy
rustup default stable
```

If `cargo` or `rustc` is not found but `~/.cargo/bin` exists, add that directory to the current shell's `PATH`:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
```

GPUI Kit's current dependency graph needs Rust 1.92 or newer. Check the version with `rustc --version`.

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

In the hosting service, protect `main`: require pull requests and the three `Rust checks` CI jobs before merging. A solo project can omit reviewer approval while still requiring CI. Do not enable auto-merge until the first CI run has succeeded.

## Start the desktop shell

From the repository root, run:

```sh
cargo run --locked --package hey-mail-desktop
```

The window is a native GPUI shell. It does not sign in or access mailbox data yet.

## Resolve and review dependencies

Run the locked check from the repository root:

```sh
cargo check --workspace --locked
```

When intentionally changing dependencies, run `cargo check --workspace` without `--locked` to update the lockfile. Review the `Cargo.lock` diff and dependency tree (`cargo tree --workspace --duplicates --locked`), then commit the lockfile. Never hand-edit it.

## Run project checks

Run these commands from the repository root:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Use `cargo fmt --all` to format Rust files. Fix Clippy findings instead of suppressing them. Add an `#[expect(..., reason = "...")]` only for a narrow lint that is demonstrably wrong.
