# Rust dependency review

Last reviewed: 2026-10-30. These notes record candidates, not accepted dependencies. Recheck versions and project fit before adding a crate.

## Review rules

When a need comes up, search for a maintained Rust library before writing general-purpose code. Use a suitable library when it meets the product, platform, and security requirements; write custom code only when the review shows that no good library fits, and record why. Before adding a direct dependency, record its purpose, current version, license, supported platforms, MSRV, maintenance activity, security history, feature graph, and API fit. Check transitive dependencies and build requirements. Compare at least one alternative when the choice affects authentication, storage, network access, or untrusted content. Add an integration proof before accepting a security-sensitive dependency.

Update this file when research changes. Record lasting decisions in an ADR. Keep dependency tasks in GitHub issues after a remote is configured.

## Candidates

| Need | Candidate and evidence | Status and next check |
| --- | --- | --- |
| Microsoft Graph client | `graph-rs-sdk` 3.0.1 supports Graph v1.0 and beta request builders, paging, and delta links. Its README example uses Tokio and it offers optional interactive browser authentication. Its latest published release was 2025-04-20. Microsoft’s supported SDK language list does not include Rust. | Evaluate against the exact folder, message delta, and calendar view delta APIs. Check current maintenance, compile it with the chosen runtime, and keep its interactive authentication feature disabled. Compare it with a small `reqwest` adapter before choosing. |
| System-browser OAuth and OpenID Connect | `oauth2` 5.0.0 supports authorization code with PKCE. The crate provides protocol building blocks, not a Microsoft-supported desktop authentication product. `openidconnect` builds on it. | Candidate for a PKCE spike only. Verify loopback redirect handling, tenant and account policy, MFA, Conditional Access, refresh, sign-out, and Windows broker requirements. |
| Operating-system credential storage | `keyring` 4.2.0 offers a cross-platform API for native secret stores. Its `keyring-core` ecosystem allows an app to choose specific store implementations. | Candidate. Verify Windows Credential Manager behavior first. Review Linux Secret Service and macOS Keychain before enabling those targets. Choose the smallest feature and backend set that meets the support plan. |
| Local SQLite storage | `rusqlite` 0.41.1 is a maintained, synchronous SQLite wrapper with transactions and bundled SQLite or SQLCipher features. | Candidate. Compare a dedicated database worker with an async SQL client. Choose migrations, encryption, key handling, backups, and deletion behavior together in a storage ADR. |
| RFC 5322 and MIME parsing | `mail-parser` 1.0.0 handles nested messages and MIME parts. Its current MSRV is Rust 1.98, above this workspace's Rust 1.92 declaration. | Defer unless the app consumes raw MIME. Microsoft Graph message JSON already returns body content. If raw MIME becomes a requirement, compare it with `mailparse` and decide whether the MSRV change is justified. |
| Untrusted HTML sanitization | `ammonia` 4.2.1 uses an HTML5 parser and a tag and attribute allowlist. It is a sanitizer, not a browser renderer. | Candidate for the email body pipeline. Configure and prove a policy that blocks scripts, remote images, remote styles, frames, forms, and unsafe links. Do not rely on library defaults without checking the rendered result. |

## Current constraints

- The application currently has no dependency on Graph, OAuth, local storage, MIME parsing, or HTML sanitization crates.
- Keep OAuth tokens out of GPUI views, logs, local mail records, and message rendering.
- Keep the main application UI in GPUI. The separate `gpui-webview` crate remains optional and needs its own security decision.
- Do not add a candidate just because it appears in this list. Recheck its current release and test the boundary it will own.

## Sources

- [Microsoft Graph SDK overview](https://learn.microsoft.com/en-us/graph/sdks/sdks-overview)
- [`graph-rs-sdk` 3.0.1 on docs.rs](https://docs.rs/crate/graph-rs-sdk/3.0.1)
- [`graph-rs-sdk` releases](https://github.com/sreeise/graph-rs-sdk/releases)
- [`oauth2` 5.0.0 documentation](https://docs.rs/oauth2/5.0.0/oauth2/)
- [`openidconnect` documentation](https://docs.rs/openidconnect/latest/openidconnect/)
- [`keyring` 4.2.0 documentation](https://docs.rs/crate/keyring/4.2.0)
- [`rusqlite` 0.41.1 documentation](https://docs.rs/crate/rusqlite/0.41.1)
- [`mail-parser` 1.0.0 documentation](https://docs.rs/crate/mail-parser/1.0.0)
- [`ammonia` 4.2.1 documentation](https://docs.rs/crate/ammonia/4.2.1)
