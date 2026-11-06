# ADR 0007: Render sanitized message HTML in native GPUI

- Status: Accepted and implemented for Linux; manual graphical evidence pending
- Date: 2026-10-31
- Owners: Project maintainer

## Context

Message bodies from Graph and the local cache are untrusted. The main application should remain native GPUI, remote content must stay blocked by default, and the message body must not receive application commands, tokens, or mailbox access.

GPUI Kit 0.8.1 already includes a native `TextView::html` renderer. It parses common HTML into GPUI's own document model, supports text selection, links, images, lists, and tables, and does not create a browser context. Its default link behavior opens URLs through GPUI, and its image handling can resolve HTTP resources, so both need an explicit application policy.

The separate `gpui-webview` crate wraps Wry and uses WKWebView on macOS, WebView2 on Windows, and WebKitGTK on Linux X11/XWayland. It is experimental, does not support Linux Wayland, and has native-child-view overlay and clipping limits on Windows. Wry exposes top-level navigation and new-window callbacks, but the reviewed GPUI Kit guidance does not establish a cross-platform policy for blocking every subresource request.

## Decision

Use Ammonia 4.2.4 for the application-owned message-body sanitization policy and GPUI Kit `TextView::html` for native rendering. Synthetic application, persistence, and headless `MailView` tests cover the production path. GPUI Kit is already part of the workspace. The former standalone HTML proof package duplicated this coverage and has been removed.

The application workflow will accept the untrusted domain body and produce an opaque `SanitizedEmailHtml` value. Keep this type and the Ammonia allowlist policy in `crates/application`; do not add GPUI or Ammonia types to `crates/domain`. Do not persist the sanitized output: sanitize when a body is prepared for display so an updated policy is applied to cached mail too.

The desktop view renders only `SanitizedEmailHtml` through GPUI Kit `TextView::html`. The allowlist keeps common semantic markup. It removes scripts, styles and style attributes, SVG, MathML, images, frames, forms, media, embeds, event attributes, and other active or remote-resource-bearing content. Reject raw HTML over 8 MiB, matching the Graph provider's response ceiling. Limit rich-render HTML to 8 MiB. If sanitized output exceeds that limit, pass at most its first 1 MiB to `html2text` 0.17.4. Cap plain text at 64 KiB and escape it inside a `pre` fragment. The escaped fragment stays below 512 KiB. Show a status when formatting or trailing content is removed. Feed the converter only Ammonia output. The converter has no link or image behavior. Open only absolute `http` or `https` links without user information after left-click, middle-click, keyboard activation, or a normal tap. Ignore right-click and touch long-press context-menu gestures. Override `TextView::image_source` so GPUI's default loader cannot fetch document resources.

Do not add `gpui-webview` for the first renderer. Revisit it in a separate security ADR only if native `TextView` fails the supported email-format acceptance checks and a cross-platform way to block scripts, downloads, and remote subresources is demonstrated. A WebView must remain a body-only leaf with no IPC bridge, app commands, tokens, or mailbox access.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Ammonia plus native GPUI `TextView` (accepted for Linux) | Reuses GPUI Kit's maintained HTML5-backed text renderer and keeps the message body in the native UI process without browser privileges. Ammonia provides a configurable allowlist and HTML5 parsing. | `TextView` supports common HTML, not full browser CSS layout. Pixel fidelity, operating-system clipboard behavior, and system-browser launch remain unverified. |
| Ammonia plus `gpui-webview` | More browser HTML and CSS fidelity while keeping the rest of the application native. | Experimental platform integration, Linux Wayland unsupported, Windows overlay/clipping limits, and no reviewed cross-platform subresource-blocking proof. A missed sanitizer case runs in a browser engine. |
| Status-only fallback | Avoids another production dependency and adds no parsing step. | Leaves an HTML-only message unreadable when its sanitized markup exceeds the rich-render limit. |
| `htmd` HTML-to-Markdown conversion | Produces readable content from HTML and uses HTML5 parsing. | Markdown syntax is not plain text and GPUI's Markdown parser can interpret links, images, and formatting. It does not meet the fallback's noninteractive text contract. |

## Consequences

- Ammonia is a production dependency of `crates/application`. Its 4.2.4 release uses HTML5 parsing, has MIT OR Apache-2.0 licensing, requires Rust 1.85, and depends on `html5ever`, `cssparser`, `maplit`, and `url`. This fits the workspace MSRV of Rust 1.95. RustSec has recorded multiple historical Ammonia vulnerabilities; 4.2.4 includes the fix for RUSTSEC-2026-0373. Keep the lockfile audit current.
- `html2text` 0.17.4 is a production dependency of `crates/application`. Its default feature set is empty. It uses `html5ever` 0.40.1, which Ammonia 4.2.4 already uses. It adds no second parser version. Its plain, non-decorating configuration adds no Markdown link footnotes. The crate is MIT licensed and requires Rust 1.85, below the workspace's 1.95 MSRV. Keep `Cargo.lock` and RustSec auditing current.
- The Ammonia configuration is Einfach Mail policy, not a replacement sanitizer. Keep its allowlist narrow and do not reimplement HTML parsing, URL parsing, or CSS sanitization.
- Verify synthetic fixtures for malformed HTML, script/event payloads, SVG and MathML mutation-XSS cases, `javascript:` and non-web links, CSS `url()` variations, remote images, frames, and forms. Include a positive fixture for common paragraphs, emphasis, lists, links, and tables.
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
- [`html2text` 0.17.4 documentation and package metadata](https://docs.rs/crate/html2text/0.17.4)
- [`html2text` 0.17.4 manifest](https://docs.rs/crate/html2text/0.17.4/source/Cargo.toml)
- [`html2text` 0.17.4 changelog](https://docs.rs/crate/html2text/0.17.4/source/CHANGELOG.md)
- [RustSec advisories for Ammonia](https://rustsec.org/packages/ammonia.html)
- [RUSTSEC-2026-0373](https://rustsec.org/advisories/RUSTSEC-2026-0373.html)
- [`htmlsanitizer` 0.2.0 documentation](https://docs.rs/htmlsanitizer/0.2.0/htmlsanitizer/)
