# GPUI Kit research

Research checked: 2026-11-13. GPUI Kit and its APIs are pre-1.0. Recheck official documentation before changing the UI dependency or adding its optional WebView crate.

## Findings

### Native application UI

GPUI Kit 0.9.1 is the latest released tag checked on 2026-11-13 and uses the Apache-2.0 license. Published on 2026-11-09, it is a backward-compatible patch with input caret/layout, soft-wrap, HTML paragraph identity, text selection, and accessibility fixes. It updates the default backend to `gpui-pre` 0.3.12 and optional GPUI Fast to 0.1.14. The `gpui-kit` crate includes matching GPUI crates, GPUI Base, the styled component library, and default icon assets. The getting-started example creates a native window with `application()`, calls `init`, then opens the app view with `open_window`.

The app depends only on `gpui-kit`. It does not include `gpui-webview` in its dependency list. The latter is a separate crate based on Wry.

GPUI Kit's published docs state Rust 1.92 or newer, but the workspace remains on Rust 1.95 based on prior compile failures in the GPUI 0.3.10 line. The GPUI Kit package manifest does not declare `rust-version`; the Linux local gate passed with the exact updated workspace lockfile on Rust 1.95. The current backend resolves to `gpui-pre` 0.3.12. GPUI Kit's verified platform notes cover macOS 15 or later, Windows 10 or later with the MSVC toolchain, and Ubuntu 24.04 packages for Linux desktop development.

The resolved workspace lockfile uses GPUI Kit 0.9.1 with `gpui-component` and `gpui-kit-assets` enabled by default, plus GPUI snapshot 0.3.12. The desktop shell uses `init` and `assets::Assets`, so both default layers are currently needed. `gpui-webview` and Wry are absent from the lockfile and the Linux feature graph.

The 0.9.0 migration notes make GPUI theme structs non-exhaustive, change `Window::text_system()` to return an `Rc`, and move animation timing to the executor clock. The app contains no affected theme struct literals, text-system calls, or direct wall-clock animation logic. Headless UI tests enable reduced motion through their shared wait helper so sheet assertions observe final geometry deterministically. Production code required no API migration, and Rust 1.95 compilation passed. The calendar composer test previously asserted DatePicker names through the local patch; ADR 0055 removes the patch and defers those names under issue #176.


The Ubuntu setup does not directly cover this Fedora 44 development host; Fedora equivalents are listed in [development setup](../project/setup.md). On 2026-10-30, `cargo check --workspace --locked --offline` and `cargo build --locked --offline --package einfach-mail-desktop` both passed. That launch attempt did not verify the window because the sandbox rejected Wayland and GPUI could not connect through X11. Fedora shell startup was verified on 2026-10-31; see [project status](../project/status.md) and [issue #1](https://github.com/1fach/einfach-mail/issues/1).

### Minimum Rust version

The workspace initially declared Rust 1.92 based on GPUI Kit's published requirement. A compile with `cargo +1.92.0 check --workspace --all-targets --locked` failed in `gpui-pre-util` 0.3.10 because it uses `[T]::as_array`, which Rust stabilized in 1.93. A compile with Rust 1.93 then failed in `gpui-pre` 0.3.10 because it uses `std::hint::cold_path`, stabilized in Rust 1.95. The exact locked workspace compiles with Rust 1.95.0 on Linux. `Cargo.toml` and CI now declare and check Rust 1.95; the Windows and macOS MSRV jobs still need a GitHub run.

### Email body rendering options

GPUI Kit's `TextView::html` is an existing native option for common message markup. The pinned 0.9.1 implementation parses HTML with `html5ever` into GPUI text and block nodes, with selection, links, images, lists, and tables. It is not a full browser or CSS layout engine. Its default link handler opens URLs externally, and its default image handling can resolve HTTP URIs and embedded data URLs; callers need explicit link and image policies for untrusted message content.

ADR 0007 selects Ammonia 4.2.4 plus native TextView for the Linux first renderer. Synthetic application tests exercise the production sanitizer against malformed markup, active and remote content, CSS URL variants, and unsafe links. Headless `MailView` tests cover link handling, selection, copy, and the oversized-body fallback. Production integration, a 1 MiB body, the 8 MiB limit, and plain text also have synthetic coverage. Pixel fidelity, OS clipboard, and system-browser launch remain unverified. The full candidate comparison and remaining acceptance evidence are in [email rendering research](email-html-rendering.md) and [ADR 0007](../decisions/0007-message-body-rendering.md).

The desktop crate enables GPUI Kit's `test-support` feature only as a development dependency for headless view tests. GPUI Fast remains outside the active Linux graph. The workspace already resolved Taffy 0.15.0 and Unicode properties 0.1.3 before the 0.9.1 upgrade, so the new release adds no version rollback for these packages. See the dependency review for the exact feature and advisory evidence.

### Optional email WebView

`gpui-webview` is a separate, experimental Wry integration. It uses WKWebView on macOS, WebView2 on Windows, and WebKitGTK on Linux X11/XWayland; Linux Wayland is unsupported. Its current guide says GPUI Fast can layer deferred overlays above the WebView on macOS and Linux, while Windows retains native child-view overlay and clipping limits. The crate must be added explicitly; it is not a dependency of the GPUI Kit facade.

The WebView remains a possible body-only renderer if native TextView fails supported email-format checks. The reviewed Wry interface provides navigation and new-window callbacks, but does not establish a cross-platform policy for blocking all subresource requests. Before adopting it, prove remote-load blocking, script/download policy, navigation behavior, and native input/focus on each supported platform. It must never receive app commands, tokens, or mailbox data.

## Sources

- [GPUI Kit getting started](https://gpui-kit.com/docs/getting-started/)
- [GPUI Kit installation](https://gpui-kit.com/docs/installation/)
- [GPUI Kit v0.9.1 release](https://github.com/longbridge/gpui-kit/releases/tag/v0.9.1)
- [GPUI Kit crate manifest](https://raw.githubusercontent.com/longbridge/gpui-kit/v0.9.1/crates/kit/Cargo.toml)
- [GPUI component Linux install script](https://github.com/longbridge/gpui-component/blob/main/script/install-linux.sh)
- [`notify-rust` 4.18.2](https://docs.rs/crate/notify-rust/4.18.2)
- [`notify-rust` 4.18.2 manifest](https://docs.rs/crate/notify-rust/4.18.2/source/Cargo.toml.orig)
- [`gpui-pre-linux` 0.3.12 manifest](https://docs.rs/crate/gpui-pre-linux/0.3.12/source/Cargo.toml)
- [`gpui-pre-util` 0.3.10 source](https://docs.rs/crate/gpui-pre-util/0.3.10/source/src/lib.rs)
- [`gpui-pre` 0.3.10 profiler source](https://docs.rs/crate/gpui-pre/0.3.10/source/src/profiler.rs)
- [Rust issue #133508: stabilize slice-to-array conversions](https://github.com/rust-lang/rust/issues/133508)
- [Rust PR #147540: stabilize `[T]::as_array`](https://github.com/rust-lang/rust/pull/147540)
- [Rust issue #136873: stabilize `cold_path`](https://github.com/rust-lang/rust/issues/136873)
- [Rust 1.93.0 release](https://blog.rust-lang.org/2026/01/22/Rust-1.93.0/)
- [Rust 1.95.0 release](https://blog.rust-lang.org/2026/04/16/Rust-1.95.0/)
- [Fedora `fontconfig-devel` package](https://packages.fedoraproject.org/pkgs/fontconfig/fontconfig-devel/)
- [Fedora `gcc-c++` package](https://packages.fedoraproject.org/pkgs/gcc/gcc-c%2B%2B/)
- [Fedora `freetype-devel` package](https://packages.fedoraproject.org/pkgs/freetype/freetype-devel/)
- [Fedora `libxkbcommon-x11-devel` package](https://packages.fedoraproject.org/pkgs/libxkbcommon/libxkbcommon-x11-devel/)
- [Fedora `wayland-protocols-devel` package](https://packages.fedoraproject.org/pkgs/wayland-protocols/wayland-protocols-devel/)
- [Fedora `vulkan-loader-devel` package](https://packages.fedoraproject.org/pkgs/vulkan-loader/vulkan-loader-devel/)
- [GPUI Kit WebView README](https://github.com/longbridge/gpui-kit/blob/v0.9.0/crates/webview/README.md)
- [GPUI WebView crate manifest](https://raw.githubusercontent.com/longbridge/gpui-kit/v0.9.0/crates/webview/Cargo.toml)
- [GPUI Kit TextView documentation](https://gpui-kit.com/base/text-view/)
- [GPUI Kit WebView guide](https://gpui-kit.com/docs/webview/)
