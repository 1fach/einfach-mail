# GPUI Kit research

Research checked: 2026-10-31. GPUI Kit and its APIs are pre-1.0. Recheck official documentation before changing the UI dependency or adding its optional WebView crate.

## Findings

### Native application UI

GPUI Kit 0.8.1 is the current release and uses the Apache-2.0 license. The `gpui-kit` crate includes matching GPUI crates, GPUI Base, the styled component library, and default icon assets. The getting-started example creates a native window with `application()`, calls `init`, then opens the app view with `open_window`.

The app depends only on `gpui-kit`. It does not include `gpui-webview` in its dependency list. The latter is a separate crate based on Wry.

GPUI Kit's published docs state Rust 1.92 or newer, but the Einfach Mail lockfile resolves GPUI snapshot 0.3.10 crates that need Rust 1.95. The app's actual minimum is therefore Rust 1.95; see [minimum Rust version](#minimum-rust-version). GPUI Kit's verified platform notes cover macOS 15 or later, Windows 10 or later with the MSVC toolchain, and Ubuntu 24.04 packages for Linux desktop development.

The resolved workspace lockfile uses GPUI Kit 0.8.1 with `gpui-component` and `gpui-kit-assets` enabled by default, plus GPUI snapshot 0.3.10. The desktop shell uses `init` and `assets::Assets`, so both default layers are currently needed. `gpui-webview` and Wry are absent from the lockfile.

The workspace has no Tauri framework, plugin, Wry, or `gpui-webview` dependency in its manifests or application source. `Cargo.lock` does contain one Tauri-named package: `tauri-winrt-notification` 0.8.1. The exact locked path is `gpui-pre-linux` 0.3.10 -> `notify-rust` 4.18.2 -> `tauri-winrt-notification`. `gpui-pre-linux` is a Linux/FreeBSD target dependency, while `notify-rust` declares `tauri-winrt-notification` only for Windows. These target conditions cannot be active together, so Cargo records the package in the cross-platform lockfile but no Einfach Mail target build selects it. The crate wraps WinRT toast notifications; it does not provide Tauri's app runtime or WebView. The CI script `scripts/check-no-tauri-target-dependencies.sh` verifies the active graph for each supported target. Removing this lockfile entry would require changing or replacing the upstream notification dependency rather than changing Einfach Mail's direct manifest. Keep the distinction explicit: the app does not use Tauri, though the generated lockfile contains this inactive Tauri-named package.

The Ubuntu setup does not directly cover this Fedora 44 development host; Fedora equivalents are listed in [development setup](../project/setup.md). On 2026-10-30, `cargo check --workspace --locked --offline` and `cargo build --locked --offline --package einfach-mail-desktop` both passed. That launch attempt did not verify the window because the sandbox rejected Wayland and GPUI could not connect through X11. Fedora shell startup was verified on 2026-10-31; see [project status](../project/status.md) and [issue #1](https://github.com/1fach/einfach-mail/issues/1).

### Minimum Rust version

The workspace initially declared Rust 1.92 based on GPUI Kit's published requirement. A compile with `cargo +1.92.0 check --workspace --all-targets --locked` failed in `gpui-pre-util` 0.3.10 because it uses `[T]::as_array`, which Rust stabilized in 1.93. A compile with Rust 1.93 then failed in `gpui-pre` 0.3.10 because it uses `std::hint::cold_path`, stabilized in Rust 1.95. The exact locked workspace compiles with Rust 1.95.0 on Linux. `Cargo.toml` and CI now declare and check Rust 1.95; the Windows and macOS MSRV jobs still need a GitHub run.

### Email body rendering options

GPUI Kit's `TextView::html` is an existing native option for common message markup. The pinned 0.8.1 implementation parses HTML with `html5ever` into GPUI text and block nodes, with selection, links, images, lists, and tables. It is not a full browser or CSS layout engine. Its default link handler opens URLs externally, and its default image handling can resolve HTTP URIs and embedded data URLs; callers need explicit link and image policies for untrusted message content.

Research proposes sanitizing message content with Ammonia 4.2.4 in the application, then rendering only a typed sanitized value through native TextView. This is not implemented or proven yet. The full candidate comparison and fixture requirements are in [email rendering research](email-html-rendering.md) and [ADR 0007](../decisions/0007-message-body-rendering.md).

### Optional email WebView

`gpui-webview` is a separate, experimental Wry integration. It uses WKWebView on macOS, WebView2 on Windows, and WebKitGTK on Linux X11/XWayland; Linux Wayland is unsupported. Its current guide says GPUI Fast can layer deferred overlays above the WebView on macOS and Linux, while Windows retains native child-view overlay and clipping limits. The crate must be added explicitly; it is not a dependency of the GPUI Kit facade.

The WebView remains a possible body-only renderer if native TextView fails supported email-format checks. The reviewed Wry interface provides navigation and new-window callbacks, but does not establish a cross-platform policy for blocking all subresource requests. Before adopting it, prove remote-load blocking, script/download policy, navigation behavior, and native input/focus on each supported platform. It must never receive app commands, tokens, or mailbox data.

## Sources

- [GPUI Kit getting started](https://gpui-kit.com/docs/getting-started/)
- [GPUI Kit installation](https://gpui-kit.com/docs/installation/)
- [GPUI Kit v0.8.1 release](https://github.com/longbridge/gpui-kit/releases/tag/v0.8.1)
- [GPUI Kit crate manifest](https://raw.githubusercontent.com/longbridge/gpui-kit/v0.8.1/crates/kit/Cargo.toml)
- [GPUI component Linux install script](https://github.com/longbridge/gpui-component/blob/main/script/install-linux.sh)
- [`notify-rust` 4.18.2](https://docs.rs/crate/notify-rust/4.18.2)
- [`notify-rust` 4.18.2 manifest](https://docs.rs/crate/notify-rust/4.18.2/source/Cargo.toml.orig)
- [`gpui-pre-linux` 0.3.10 manifest](https://docs.rs/crate/gpui-pre-linux/0.3.10/source/Cargo.toml)
- [`gpui-pre-util` 0.3.10 source](https://docs.rs/crate/gpui-pre-util/0.3.10/source/src/lib.rs)
- [`gpui-pre` 0.3.10 profiler source](https://docs.rs/crate/gpui-pre/0.3.10/source/src/profiler.rs)
- [Rust issue #133508: stabilize slice-to-array conversions](https://github.com/rust-lang/rust/issues/133508)
- [Rust PR #147540: stabilize `[T]::as_array`](https://github.com/rust-lang/rust/pull/147540)
- [Rust issue #136873: stabilize `cold_path`](https://github.com/rust-lang/rust/issues/136873)
- [Rust 1.93.0 release](https://blog.rust-lang.org/2026/01/22/Rust-1.93.0/)
- [Rust 1.95.0 release](https://blog.rust-lang.org/2026/04/16/Rust-1.95.0/)
- [`tauri-winrt-notification` 0.8.1 package metadata](https://docs.rs/crate/tauri-winrt-notification/0.8.1)
- [Fedora `fontconfig-devel` package](https://packages.fedoraproject.org/pkgs/fontconfig/fontconfig-devel/)
- [Fedora `gcc-c++` package](https://packages.fedoraproject.org/pkgs/gcc/gcc-c%2B%2B/)
- [Fedora `freetype-devel` package](https://packages.fedoraproject.org/pkgs/freetype/freetype-devel/)
- [Fedora `libxkbcommon-x11-devel` package](https://packages.fedoraproject.org/pkgs/libxkbcommon/libxkbcommon-x11-devel/)
- [Fedora `wayland-protocols-devel` package](https://packages.fedoraproject.org/pkgs/wayland-protocols/wayland-protocols-devel/)
- [Fedora `vulkan-loader-devel` package](https://packages.fedoraproject.org/pkgs/vulkan-loader/vulkan-loader-devel/)
- [GPUI Kit WebView README](https://github.com/longbridge/gpui-kit/blob/v0.8.1/crates/webview/README.md)
- [GPUI WebView crate manifest](https://raw.githubusercontent.com/longbridge/gpui-kit/v0.8.1/crates/webview/Cargo.toml)
- [GPUI Kit TextView documentation](https://gpui-kit.com/base/text-view/)
- [GPUI Kit WebView guide](https://gpui-kit.com/docs/webview/)
