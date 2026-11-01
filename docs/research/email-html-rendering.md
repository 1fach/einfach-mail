# Email HTML rendering research

Research checked: 2026-10-31. ADR 0007 accepts Ammonia and GPUI Kit `TextView::html` for Linux. Production integration is implemented; pixel fidelity, OS clipboard, and system-browser checks remain open. Recheck library releases and advisories when updating production dependencies.

## Requirements

- Treat provider and cached message bodies as untrusted, including malformed HTML.
- Keep the application shell and message body view in GPUI where practical.
- Block remote images, scripts, stylesheets, frames, and other automatic network requests by default.
- Keep tokens, application commands, IPC, and mailbox access outside message content.
- Preserve useful message formatting, links, tables, selection, and copy behavior.
- Use maintained parsing and sanitization libraries instead of writing a general-purpose parser or sanitizer.

## Sanitizer candidates

### Ammonia 4.2.4

Ammonia 4.2.4 was published on 2026-10-17. It parses and serializes fragments with `html5ever` according to HTML5 parsing rules, and provides an explicit builder allowlist. Its license is MIT OR Apache-2.0, its MSRV is Rust 1.85, and its runtime dependencies are `html5ever` 0.40, `cssparser` 0.38, `maplit` 1, and `url` 2. The MSRV fits this workspace's Rust 1.95 declaration.

RustSec lists multiple historical Ammonia advisories, including recent XSS, mutation-XSS, and CSS URL-filtering issues. The current 4.2.4 release patches RUSTSEC-2026-0373; earlier 4.x security fixes are recorded in its changelog. This security history makes lockfile auditing and a deliberately small allowlist important. Do not use Ammonia's defaults as Einfach Mail policy.

### `htmlsanitizer` 0.2.0

`htmlsanitizer` is an MIT-licensed, allowlist-based DFA sanitizer. It was published on 2026-03-27 and has three published Rust releases. Its documented runtime dependency is `url`, with optional `js-sys` and `wasm-bindgen` for WASM. The reviewed metadata does not declare an MSRV. It has a fuzz target, but its own documentation says it is not a full HTML parser and instead conservatively handles common input. No RustSec package advisory entry was found during this review; that absence is not evidence of a security audit.

This is a useful alternative for low-cost streaming sanitization, but Ammonia is the stronger fit for malformed email HTML because it uses a browser-compatible HTML5 parser and has a longer security and release history. Do not implement a custom parser or compose lower-level parser crates into a custom sanitizer.

## Renderer options

| Option | Fit | Limits |
| --- | --- | --- |
| GPUI Kit `TextView::html` | Native GPUI renderer already included by the pinned `gpui-kit` dependency. It parses common HTML into GPUI content and supports selection, links, images, lists, and tables. It does not create a browser context. | It is not a full HTML/CSS browser. Its default link action calls `App::open_url`; its default image handling can resolve HTTP resources and embedded `data:` images. Override link handling and image resolution, and remove remote image markup from the initial sanitizer profile. |
| `gpui-webview` plus Ammonia | Better browser HTML and CSS fidelity while leaving the rest of the app native. | Experimental, separate dependency. Linux requires X11/XWayland; Windows native bounds can cover GPUI overlays and escape clipping. Navigation callbacks do not by themselves establish subresource blocking. Downloads also need an explicit policy. Do not use until those controls are proven on each supported OS. |
| `html2text` 0.17.4 fallback | Maintained HTML5-based converter that emits plain text. Its plain non-decorating configuration adds no link footnotes. The application feeds it only Ammonia-sanitized content and the UI treats the result as escaped selectable text. | Removes rich formatting and is bounded to the first 1 MiB of sanitized markup and 64 KiB of plain text. A failed conversion or an over-limit raw body gets a status message. |

GPUI Kit's native `TextView` behavior was checked in the 0.8.1 source. The HTML parser uses `html5ever`; `<a>` nodes become link marks and `<img>` nodes become image nodes. Without a custom link handler, links open through `App::open_url`. Without a custom image resolver, image nodes use GPUI's URI/data URL behavior; the image resolver API is authoritative and does not fall back to the document URL when overridden. Therefore, native rendering still needs the sanitizer and a link/image policy.

The current GPUI Kit WebView guide documents WKWebView on macOS, WebView2 on Windows, and WebKitGTK on Linux X11. Linux Wayland is unsupported. GPUI Fast supports deferred overlays above WebViews on macOS and Linux; Windows retains child-view overlay and clipping limits. Wry exposes page navigation and new-window callbacks, but the reviewed guidance does not document a cross-platform hook to block all remote subresource requests. That is an evidence gap, not a claim that platform-specific interception is impossible.

## Accepted Linux first-renderer pipeline

1. Keep raw provider and cached body values untrusted.
2. In `crates/application`, sanitize each HTML body prepared for display with Ammonia 4.2.4 and the product allowlist, producing an opaque `SanitizedEmailHtml` value. Do not persist the sanitized output. Reject raw input over 8 MiB, matching the Graph provider's per-response ceiling, and limit rich-render HTML to 8 MiB.
3. Allow common semantic tags supported by TextView. Strip scripts, style tags and style attributes, SVG, MathML, images, frames, forms, media, embedded content, event attributes, and unsupported active markup. Restrict links to `http` and `https`.
4. If sanitized output exceeds the rich-render limit, pass at most its first 1 MiB to `html2text` 0.17.4. Cap the plain result at 64 KiB. Ammonia escaping expands text at most sevenfold, keeping the `pre` fragment below 512 KiB. Feed the converter only sanitized input. Enable no CSS or Markdown decoration. If conversion fails, keep a clear reader status.
5. Render `SanitizedEmailHtml` with `TextView::html`. Validate clicked URLs again. Open only HTTP or HTTPS links without user information. Override `image_source` so it resolves no images. Render the fallback as escaped text in a selectable native `TextView`. Show when formatting or trailing content was removed.
6. Keep `gpui-webview` out of the first renderer. Reconsider it in a separate security ADR only if the native view fails supported mail-format checks and remote subresource blocking is proven across the supported platforms.

## Remaining evidence

- Run Ammonia 4.2.4 against synthetic fixtures for malformed markup, scripts and event attributes, SVG/MathML mutation-XSS inputs, unsafe schemes, CSS URL case variations, remote images, frames, and forms.
- Verify positive rendering for common paragraphs, emphasis, lists, links, and tables using synthetic content.
- Verify link clicks reject every scheme except the allowed web schemes and never route content into app commands.
- Verify the native renderer makes no automatic network requests for images, stylesheets, frames, or other content.
- Verify copy, selection, large body behavior, and failure fallback on each supported target.
- Run a RustSec audit against the resolved lockfile before adding Ammonia.

## Sources

- [`ammonia` 4.2.4 package metadata and docs](https://docs.rs/crate/ammonia/4.2.4)
- [`ammonia` changelog](https://github.com/rust-ammonia/ammonia/blob/master/CHANGELOG.md)
- [RustSec's Ammonia advisory list](https://rustsec.org/packages/ammonia.html)
- [RUSTSEC-2026-0373](https://rustsec.org/advisories/RUSTSEC-2026-0373.html)
- [`htmlsanitizer` 0.2.0 package metadata and docs](https://docs.rs/crate/htmlsanitizer/0.2.0)
- [`html2text` 0.17.4 documentation and package metadata](https://docs.rs/crate/html2text/0.17.4)
- [`html2text` 0.17.4 manifest](https://docs.rs/crate/html2text/0.17.4/source/Cargo.toml)
- [`html2text` 0.17.4 changelog](https://docs.rs/crate/html2text/0.17.4/source/CHANGELOG.md)
- [GPUI Kit TextView docs](https://gpui-kit.com/base/text-view/)
- [GPUI Kit v0.8.1 TextView source](https://github.com/longbridge/gpui-kit/blob/v0.8.1/crates/base/src/text/text_view.rs)
- [GPUI Kit v0.8.1 HTML parser source](https://github.com/longbridge/gpui-kit/blob/v0.8.1/crates/base/src/text/format/html.rs)
- [GPUI Kit WebView guide](https://gpui-kit.com/docs/webview/)
- [GPUI Kit v0.8.1 WebView manifest](https://raw.githubusercontent.com/longbridge/gpui-kit/v0.8.1/crates/webview/Cargo.toml)
