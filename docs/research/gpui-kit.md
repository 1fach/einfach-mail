# GPUI Kit research

Research checked: 2026-10-30. GPUI Kit and its APIs are pre-1.0. Recheck official documentation before changing the UI dependency or adding its optional WebView crate.

## Findings

### Native application UI

GPUI Kit 0.8.1 is the current release and uses the Apache-2.0 license. The `gpui-kit` crate includes matching GPUI crates, GPUI Base, the styled component library, and default icon assets. The getting-started example creates a native window with `application()`, calls `init`, then opens the app view with `open_window`.

The app depends only on `gpui-kit`. It does not include `gpui-webview` in its dependency list. The latter is a separate crate based on Wry.

GPUI Kit's current docs require Rust 1.92 or newer for its locked dependency graph. Their verified platform notes cover macOS 15 or later, Windows 10 or later with the MSVC toolchain, and Ubuntu 24.04 packages for Linux desktop development.

The resolved workspace lockfile uses GPUI Kit 0.8.1 with `gpui-component` and `gpui-kit-assets` enabled by default, plus GPUI snapshot 0.3.10. The desktop shell uses `init` and `assets::Assets`, so both default layers are currently needed. `gpui-webview` and Wry are absent from the lockfile.

The lockfile contains one Tauri-named package, `tauri-winrt-notification` 0.8.1. It is declared by `notify-rust` as a Windows-only backend; GPUI's Linux backend also uses `notify-rust`, which causes Cargo to record that conditional package. The current Linux `cargo tree` has no path to it. Its lock entry contains Windows API dependencies, not a Tauri application runtime or WebView. No Tauri framework dependency is present in the workspace manifests.

The Ubuntu setup does not directly cover this Fedora 44 development host; Fedora equivalents are listed in [development setup](../project/setup.md). A locked offline build now reaches native compilation but has not passed because the host lacks development packages, including Fontconfig and a C++ compiler.

### Optional email WebView

`gpui-webview` is marked experimental. Its current README lists WKWebView on macOS, WebView2 on Windows, and WebKitGTK on Linux X11/XWayland. Linux Wayland is unsupported. The native view covers GPUI overlays in its bounds on Windows. The crate must be added explicitly; it is not a dependency of the GPUI Kit facade.

This makes the WebView a possible leaf renderer for sanitized email HTML, not a good fit for the main app UI. Before adopting it, select a maintained HTML parser and sanitizer, block remote loads and scripts, disable navigation and privileged APIs, and test the result on the supported windowing systems.

## Sources

- [GPUI Kit getting started](https://gpui-kit.com/docs/getting-started/)
- [GPUI Kit installation](https://gpui-kit.com/docs/installation/)
- [GPUI Kit v0.8.1 release](https://github.com/longbridge/gpui-kit/releases/tag/v0.8.1)
- [GPUI Kit crate manifest](https://raw.githubusercontent.com/longbridge/gpui-kit/v0.8.1/crates/kit/Cargo.toml)
- [GPUI component Linux install script](https://github.com/longbridge/gpui-component/blob/main/script/install-linux.sh)
- [`notify-rust` 4.18.2](https://docs.rs/crate/notify-rust/4.18.2)
- [Fedora `fontconfig-devel` package](https://packages.fedoraproject.org/pkgs/fontconfig/fontconfig-devel/)
- [Fedora `gcc-c++` package](https://packages.fedoraproject.org/pkgs/gcc/gcc-c%2B%2B/)
- [Fedora `freetype-devel` package](https://packages.fedoraproject.org/pkgs/freetype/freetype-devel/)
- [Fedora `libxkbcommon-x11-devel` package](https://packages.fedoraproject.org/pkgs/libxkbcommon/libxkbcommon-x11-devel/)
- [Fedora `wayland-protocols-devel` package](https://packages.fedoraproject.org/pkgs/wayland-protocols/wayland-protocols-devel/)
- [Fedora `vulkan-loader-devel` package](https://packages.fedoraproject.org/pkgs/vulkan-loader/vulkan-loader-devel/)
- [GPUI Kit WebView README](https://github.com/longbridge/gpui-kit/blob/v0.8.1/crates/webview/README.md)
- [GPUI WebView crate manifest](https://raw.githubusercontent.com/longbridge/gpui-kit/v0.8.1/crates/webview/Cargo.toml)
