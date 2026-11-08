# ADR 0001: Use GPUI Kit for the native desktop shell

- Status: Accepted
- Date: 2026-10-30

## Context

Einfach Mail is a Rust desktop application. Its main window, navigation, mail list, calendar, and settings should use native Rust UI. Rich email bodies may need an HTML renderer, but that renderer should not own the rest of the application.

GPUI Kit packages GPUI, its base layer, styled components, and default assets behind one Rust dependency. Its optional `gpui-webview` crate is separate from the `gpui-kit` dependency.

## Decision

Use GPUI Kit 0.9.0 for the application shell and native views. Keep workflows in the application crate and domain rules in the domain crate.

The 2026-11-08 review found no affected theme struct literals or calls to `Window::text_system()`. App code contains no wall-clock animation logic. Headless UI tests enable reduced motion in the shared wait helper, so sheet assertions check final geometry. The Rust 1.95 workspace check passed, and the event-composer test preserved both DatePicker accessible names. GPUI Kit is pre-1.0. Review API changes with each update.

If sanitized HTML email later needs a browser engine, evaluate `gpui-webview` only for an isolated message-body view and record a separate security decision before adding it. Keep scripts, remote content, navigation, and access to application data disabled.

## Consequences

- `apps/desktop/` owns the GPUI application lifecycle, views, keyboard actions, and OS integration.
- Rust remains the language for application logic and the primary UI.
- GPUI Kit is pre-1.0 software. Keep the resolved dependency graph in `Cargo.lock` and review API changes when updating it.
- GPUI Kit's optional WebView crate is experimental. Its current docs list macOS, Windows, and Linux X11/XWayland support; Linux Wayland is unsupported. On Windows the WebView covers GPUI overlays in its bounds. Recheck these limits before an email renderer is chosen.
- The desktop shell and email renderer have separate trust boundaries. Message content must never gain access to application commands, tokens, or local mailbox data.

## Sources

- [GPUI Kit getting started](https://gpui-kit.com/docs/getting-started/)
- [GPUI Kit v0.9.0 release](https://github.com/longbridge/gpui-kit/releases/tag/v0.9.0)
- [GPUI Kit WebView support](https://github.com/longbridge/gpui-kit/blob/v0.9.0/crates/webview/README.md)
