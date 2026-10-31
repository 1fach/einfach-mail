# ADR 0007: Render sanitized message HTML in native GPUI

- Status: Proposed; test-only sanitizer proof passes; renderer proof required
- Date: 2026-10-30
- Owners: Project maintainer

## Context

Message bodies from Graph and the local cache are untrusted. The main application should remain native GPUI, remote content must stay blocked by default, and the message body must not receive application commands, tokens, or mailbox access.

GPUI Kit 0.8.1 already includes a native `TextView::html` renderer. It parses common HTML into GPUI's own document model, supports text selection, links, images, lists, and tables, and does not create a browser context. Its default link behavior opens URLs through GPUI, and its image handling can resolve HTTP resources, so both need an explicit application policy.

The separate `gpui-webview` crate wraps Wry and uses WKWebView on macOS, WebView2 on Windows, and WebKitGTK on Linux X11/XWayland. It is experimental, does not support Linux Wayland, and has native-child-view overlay and clipping limits on Windows. Wry exposes top-level navigation and new-window callbacks, but the reviewed GPUI Kit guidance does not establish a cross-platform policy for blocking every subresource request.

## Decision

Propose Ammonia 4.2.4 for application-owned message-body sanitization. Use GPUI Kit `TextView::html` for native rendering. Keep Ammonia in a test-only proof package until the sanitizer and renderer acceptance checks below pass. Do not add Ammonia to the application crate or integrate the renderer yet. GPUI Kit is already in the workspace.

The application workflow will accept the untrusted domain body and produce an opaque `SanitizedEmailHtml` value. Keep this type and the Ammonia allowlist policy in `crates/application`; do not add GPUI or Ammonia types to `crates/domain`. Do not persist the sanitized output: sanitize when a body is prepared for display so an updated policy is applied to cached mail too.

The desktop view will render only `SanitizedEmailHtml` through GPUI Kit `TextView::html`. Start with an explicit allowlist for common semantic message markup. Strip scripts, styles and style attributes, SVG, MathML, images, frames, forms, media, embeds, event attributes, and other active or remote-resource-bearing content. Allow only `http` and `https` link targets, and install a `TextView::on_link_click` handler that opens only those user-clicked links externally. Do not use TextView's default URL handler. If image support is added later, override `TextView::image_source` so remote URLs cannot fall through to GPUI's resource loader; resolve only approved app-owned content such as a validated inline attachment.

Do not add `gpui-webview` for the first renderer. Revisit it in a separate security ADR only if native `TextView` fails the supported email-format acceptance checks and a cross-platform way to block scripts, downloads, and remote subresources is demonstrated. A WebView must remain a body-only leaf with no IPC bridge, app commands, tokens, or mailbox access.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Ammonia plus native GPUI `TextView` (proposed) | Reuses GPUI Kit's maintained HTML5-backed text renderer and keeps the message body in the native UI process without browser privileges. Ammonia provides a configurable allowlist and HTML5 parsing. | `TextView` supports common HTML, not full browser CSS layout. Its default links and image handling need explicit overrides. The current tests cover a small fixture set. Adversarial coverage and renderer behavior remain unverified. |
| Ammonia plus `gpui-webview` | More browser HTML and CSS fidelity while keeping the rest of the application native. | Experimental platform integration, Linux Wayland unsupported, Windows overlay/clipping limits, and no reviewed cross-platform subresource-blocking proof. A missed sanitizer case runs in a browser engine. |
| Plain-text-only body | Smallest active-content surface and no HTML sanitizer dependency. | Loses useful formatting, tables, and links for HTML-only messages. It remains a fallback when a body cannot be rendered safely. |

## Consequences

- Ammonia is the only proposed parser/sanitizer dependency. Its 4.2.4 release uses HTML5 parsing, has MIT OR Apache-2.0 licensing, requires Rust 1.85, and depends on `html5ever`, `cssparser`, `maplit`, and `url`. This fits the workspace MSRV of Rust 1.95. RustSec has recorded multiple historical Ammonia vulnerabilities; 4.2.4 includes the fix for RUSTSEC-2026-0373. Recheck the advisory database and audit the resolved lockfile before accepting the dependency.
- The Ammonia configuration is Einfach Mail policy, not a replacement sanitizer. Keep its allowlist narrow and do not reimplement HTML parsing, URL parsing, or CSS sanitization.
- The test-only package at [`tools/message-body-proof`](../../tools/message-body-proof/) passes five synthetic tests on Rust 1.95. The fixtures cover common formatting, active content, remote-resource elements, and event, style, and navigation attributes. They also cover non-web links, malformed HTML, CSS URL variations, and malformed SVG and MathML namespace payloads. These tests confirm only the included fixtures. They do not verify GPUI rendering or cover all mutation-XSS cases.
- Review the remaining mutation-XSS and URL parser cases before accepting Ammonia in the application crate. Keep the dependency in the proof package until that review passes.
- Verify the actual rendered body does not load remote images or other network resources, does not open unsafe URLs, and keeps copy and selection usable. Never use real mailbox content in fixtures.
- Browser rendering remains a separate decision. If selected later, prove remote-resource blocking and navigation policy on every supported OS before exposing real message bodies to it.

## Sources

- [GPUI Kit TextView documentation](https://gpui-kit.com/base/text-view/)
- [GPUI Kit v0.8.1 TextView implementation](https://github.com/longbridge/gpui-kit/blob/v0.8.1/crates/base/src/text/text_view.rs)
- [GPUI Kit v0.8.1 HTML parser implementation](https://github.com/longbridge/gpui-kit/blob/v0.8.1/crates/base/src/text/format/html.rs)
- [GPUI Kit WebView guide](https://gpui-kit.com/docs/webview/)
- [GPUI Kit v0.8.1 WebView manifest](https://raw.githubusercontent.com/longbridge/gpui-kit/v0.8.1/crates/webview/Cargo.toml)
- [`ammonia` 4.2.4 documentation and package metadata](https://docs.rs/crate/ammonia/4.2.4)
- [`ammonia` changelog](https://github.com/rust-ammonia/ammonia/blob/master/CHANGELOG.md)
- [RustSec advisories for Ammonia](https://rustsec.org/packages/ammonia.html)
- [RUSTSEC-2026-0373](https://rustsec.org/advisories/RUSTSEC-2026-0373.html)
- [`htmlsanitizer` 0.2.0 documentation](https://docs.rs/htmlsanitizer/0.2.0/htmlsanitizer/)
