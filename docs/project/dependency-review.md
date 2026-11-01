# Rust dependency review

Last reviewed: 2026-11-01. These notes record dependency evidence and adopted choices. Recheck versions and project fit before adding or upgrading a crate.

## Review rules

When a need comes up, search for a maintained Rust library before writing general-purpose code. Use a suitable library when it meets the product, platform, and security requirements; write custom code only when the review shows that no good library fits, and record why. Before adding a direct dependency, record its purpose, current version, license, supported platforms, MSRV, maintenance activity, security history, feature graph, and API fit. Check transitive dependencies and build requirements. Compare at least one alternative when the choice affects authentication, storage, network access, or untrusted content. Add an integration proof before accepting a security-sensitive dependency.

Update this file when research changes. Record lasting decisions in an ADR. Keep dependency tasks in GitHub issues after a remote is configured.

Linux is the only active development target. Evaluate current candidates for Linux; record Windows and macOS gaps for their later support milestones.

## Development tools

| Need | Tool and evidence | Project choice |
| --- | --- | --- |
| Rust lockfile advisory audit | [`cargo-audit` 0.22.2](https://github.com/rustsec/rustsec/blob/main/cargo-audit/Cargo.toml) is the RustSec tool for auditing `Cargo.lock` against the [RustSec Advisory Database](https://github.com/RustSec/advisory-db). It is actively maintained, licensed MIT OR Apache-2.0, and declares Rust 1.88 as its MSRV. Its default `binary-scanning` feature is not needed for this lockfile check. | Pin 0.22.2 in a separate read-only CI workflow and install with `--no-default-features`; run `cargo audit --deny yanked` so yanked locked versions fail CI. The official [`rustsec/audit-check` action](https://github.com/rustsec/audit-check) takes a GitHub token and documents check/issue write permissions; direct CLI execution keeps this workflow at `contents: read`. `cargo-deny` is used separately for exact package bans; keep RustSec advisory auditing in `cargo-audit`. |
| Tauri and WebView dependency ban | [`cargo-deny` 0.20.2](https://github.com/EmbarkStudios/cargo-deny/blob/main/Cargo.toml) is actively maintained, MIT OR Apache-2.0, and declares Rust 1.88 as its MSRV. The source manifest has 32 direct dependencies and its published lockfile has 211 packages. Its `bans` check supports exact package bans with reasons; it does not provide crate-name prefix wildcards. | Pin 0.20.2 in the workspace dependency policy CI job and run `cargo deny check bans` for the active Linux targets with all workspace features. `deny.toml` explicitly bans Tauri runtime/build crates, Wry, and `gpui-webview`. Keep `tauri-winrt-notification` unbanned: it appears in the generated cross-platform lockfile through GPUI's Linux notification backend, but its Windows-only edge cannot combine with GPUI's Linux-only backend on an actual target. It wraps WinRT notifications and does not provide Tauri's app runtime or WebView. `scripts/check-no-tauri-target-dependencies.sh` checks the active Linux host target by default and fails if its graph contains any Tauri-prefixed, Wry, or GPUI WebView package. Pass an explicit target triple for later platform checks. Use `cargo-audit` for RustSec advisory and yanked-version policy; do not configure cargo-deny's unrelated license, source, or duplicate-version checks without a reviewed policy. |
| Workspace dependency-boundary check | [`cargo-depgraph-check` 0.1.0](https://docs.rs/crate/cargo-depgraph-check/0.1.0) is an MIT-licensed Cargo subcommand with Rust 1.85 MSRV. It reads Cargo's resolved dependency graph and enforces direct internal workspace allowlists, including strict mode for unlisted members and an option to check dev-dependencies. Its direct dependencies are `cargo_metadata`, `clap`, `toml`, `serde`, `serde_json`, and `thiserror`. | Keep 0.1.0 pinned with `--locked` in the Linux CI job and validate crate boundaries in `depgraph-rules.toml`. It is a young tool (0.1.0, 18 repository commits, two stars at review time), so keep the pin and review maintenance, platform support, and RustSec history before upgrading. Exact direct-edge allowlists match this workspace's boundary policy. An offline RustSec scan of its published lockfile (105 packages) reported `anyhow` 1.0.102 under [RUSTSEC-2026-0190](https://rustsec.org/advisories/RUSTSEC-2026-0190.html), patched in 1.0.103; the advisory affects `Error::downcast_mut`, which is not called by the checker or `cargo_metadata` 0.19.2. The temporary registry index lacked metadata for several packages, so yanked-version status remains unverified. Compared alternatives: [`cargo-arc` 0.6.2](https://docs.rs/crate/cargo-arc/0.6.2) adds module-level analysis and layer rules under MIT OR Apache-2.0, but its published lockfile has 392 packages and installation warned that `spin` 0.9.8 is yanked. Its module graph also omits modules declared by macros, excludes `cfg(test)` by default, and does not evaluate other `cfg` attributes; its ordered layers need additional forbidden-edge rules to encode exact direct-edge allowlists. [`cargo-bylaw`](https://github.com/danielgerlag/cargo-bylaw/blob/main/Cargo.toml) offers broader semantic module rules but its current manifest requires Rust 1.95 and includes rust-analyzer crates, which is unnecessary cost for this crate-level policy. `cargo-deny` does not express per-crate internal edge allowlists ([open feature request](https://github.com/EmbarkStudios/cargo-deny/issues/891)). |

### Workspace MSRV validation — 2026-10-31

The workspace initially declared Rust 1.92 based on GPUI Kit's published installation requirement. A real compile showed that the resolved GPUI 0.3.10 snapshot uses newer standard-library APIs: Rust 1.92 fails at `[T]::as_array`, stabilized in 1.93, and Rust 1.93 fails at `std::hint::cold_path`, stabilized in 1.95. The workspace `rust-version` and CI minimum are 1.95. Current development validation targets Linux; Windows and macOS build and runtime checks are deferred to their support milestones.

### Initial lockfile audit — 2026-10-31

`cargo-audit` 0.22.2 loaded 1,395 RustSec advisories and scanned 797 locked crate dependencies. It found no known vulnerable versions. It reported these unmaintained crates:

- `instant` 0.1.13 — RUSTSEC-2024-0384
- `paste` 1.0.15 — RUSTSEC-2024-0436
- `rustls-pemfile` 2.2.0 — RUSTSEC-2025-0134
- `rustybuzz` 0.20.1 — RUSTSEC-2026-0206
- `ttf-parser` 0.25.1 — RUSTSEC-2026-0192
- `zbus_names` 4.3.5 — RUSTSEC-2026-0371
- `zvariant` 5.15.1 — RUSTSEC-2026-0370
- `zvariant_derive` 5.15.1 — RUSTSEC-2026-0372

These crates are transitive lockfile entries, not direct workspace dependencies. The first audit run used a temporary `CARGO_HOME` and could not locate `cargo` because `~/.cargo/bin` is not on this shell's `PATH`; it therefore could not update the crates.io index. A follow-up run set `CARGO` to the installed Cargo binary and completed `cargo audit --deny yanked`, updating the index successfully. It emitted no vulnerable-version or yanked-version warnings. No advisory was ignored. The RustSec workflow passed against the branch's committed lockfile in [PR #13 run 41155947067](https://github.com/1fach/einfach-mail/actions/runs/41155947067). [Issue #10](https://github.com/1fach/einfach-mail/issues/10) remains open until the workflow lands on `main` and passes there.

### M1 candidate lockfile audit — 2026-11-01

`cargo audit --db /tmp/hey-mail-rustsec-advisory-db --deny yanked` loaded 1,395 advisories and scanned 889 locked crate dependencies. It found no known vulnerable versions or yanked crates and reported the same eight upstream unmaintained transitive crates listed above. It reported no advisory for `tokio-util` 0.7.20. No advisory was ignored. The temporary advisory database was removed after the audit.

### M2 local candidate lockfile audit — 2026-11-01

`cargo audit --db /tmp/m2-read-only-mail-rustsec-db --deny yanked` loaded 1,395 advisories and scanned 916 locked crate dependencies. It found no known vulnerable versions or yanked crates and reported the same eight upstream unmaintained transitive crates listed above. No advisory was ignored. The temporary advisory database is kept outside the repository and removed after the local review.

### Unmaintained warning triage — 2026-10-31

All eight findings are RustSec `INFO Unmaintained` advisories. The audit found no known vulnerable versions. The Linux dependency paths were traced with `cargo tree`: the findings are owned by GPUI Kit, its rendering stack, and Linux desktop integration crates; Einfach Mail does not call these crates directly. `rustls-pemfile` is present through `gpui-pre-reqwest`, but it is not active in the default Linux tree. Offline Windows and macOS tree inspection could not finish because the corresponding platform crates are not cached.

| Crate | Current dependency path | RustSec guidance and disposition |
| --- | --- | --- |
| `instant` | `gpui-base` and `gpui-component` | RustSec recommends `web-time`. Follow the GPUI upstream dependency update; do not add a direct alias to this app. ([RUSTSEC-2024-0384](https://rustsec.org/advisories/RUSTSEC-2024-0384.html)) |
| `paste` | `gpui-component` and the GPUI image pipeline | RustSec lists `pastey` as a drop-in fork and `with_builtin_macros` as a broader alternative. Keep this transitive until upstream adopts a compatible replacement; do not patch the shared dependency graph without a compatibility proof. ([RUSTSEC-2024-0436](https://rustsec.org/advisories/RUSTSEC-2024-0436.html)) |
| `rustls-pemfile` | `gpui-pre-reqwest` in the lockfile; no active Linux path | Rustls recommends using `PemObject` from `rustls-pki-types`. Track the owning crate's migration; Einfach Mail has no direct PEM parsing need. ([RUSTSEC-2025-0134](https://rustsec.org/advisories/RUSTSEC-2025-0134.html)) |
| `rustybuzz` | `usvg` and the GPUI renderer | RustSec lists `harfrust`. This is a renderer-level dependency change, so follow a compatible `usvg`/GPUI update rather than replacing the shaping engine in the app. ([RUSTSEC-2026-0206](https://rustsec.org/advisories/RUSTSEC-2026-0206.html)) |
| `ttf-parser` | `fontdb`, `usvg`, and the GPUI renderer | RustSec lists `skrifa`. The font stack must migrate coherently upstream; Einfach Mail does not parse fonts directly. ([RUSTSEC-2026-0192](https://rustsec.org/advisories/RUSTSEC-2026-0192.html)) |
| `zbus_names`, `zvariant`, `zvariant_derive` | Linux D-Bus, accessibility, portal, and notification dependencies; current lockfile uses `zbus` 5.19.0 | RustSec says these APIs moved into `zbus` 6.0.0 (`names`, `wire`, and derive macros; use `zgvariant` for GVariant). Upgrade the Linux integration stack as a compatible unit. ([RUSTSEC-2026-0371](https://rustsec.org/advisories/RUSTSEC-2026-0371.html), [RUSTSEC-2026-0370](https://rustsec.org/advisories/RUSTSEC-2026-0370.html), [RUSTSEC-2026-0372](https://rustsec.org/advisories/RUSTSEC-2026-0372.html)) |

No direct replacement or audit suppression is added for M0. These maintenance warnings remain an upstream dependency risk; revisit them when GPUI Kit or its platform/rendering dependencies publish compatible updates. The local and PR #13 strict audits found no vulnerable or yanked crates. Keep issue #10 open until the workflow lands on `main`.

## Candidates

| Need | Candidate and evidence | Status and next check |
| --- | --- | --- |
| App-generated entity identifiers | `uuid` 1.29.0 is actively developed under Apache-2.0 or MIT and has a Rust 1.89 MSRV. UUID v4 generation is feature-gated and uses operating-system randomness; v7 adds timestamp ordering. Both fit this workspace's MSRV. | Use `uuid` with defaults disabled and only `std` to parse the Entra client ID. Do not generate app IDs; account identity remains the opaque Graph `id` under ADR 0009. |
| Microsoft Graph client | Microsoft does not list Rust among its Graph SDK languages or Kiota generators. `graph-rs-sdk` 3.0.1 has broad Graph coverage, paging, and delta support. Its latest stable release is dated 2025-04-20. It uses `reqwest` 0.12 and Tokio. Its optional `interactive-auth` feature adds Wry and Tao. `io-msgraph` 0.6.0 has an I/O-free Rust core and broad mail/calendar operations, but its error type drops `Retry-After`, its source logs request URLs at `trace` level, and its published README conflicts with its changelog. `reqwest` 0.13.5 is MIT or Apache-2.0, supports Rust 1.85 or later, and exposes status and headers. Its async client requires Tokio, while GPUI Kit uses `smol` in the active Linux dependency graph. | ADR 0006 accepts `reqwest` and `serde` for the Linux Graph adapter. The provider owns a private Tokio worker, HTTP and Graph types, response limits, and token handling. The M2 implementation uses read-only folder, message, delta, and fetch-on-open body requests. Issue #7's local synthetic transport proof is complete; live Graph behavior remains unverified. Calendar operations are outside this candidate. |
| Microsoft 365 desktop authentication | Microsoft's published MSAL list does not include Rust. The Microsoft `azure_identity` crate documents developer-tool, Azure-hosted, and service-principal credentials, not an interactive desktop public-client flow. `oauth2` 5.0.0 (MIT OR Apache-2.0, Rust 1.65) implements authorization-code with PKCE and refresh. `openidconnect` 4.0.1 (MIT, Rust 1.65) adds discovery and ID-token validation, but does not validate `azp`. It also depends on `rsa`, which has an unpatched RustSec Marvin timing advisory. Neither protocol crate starts the system browser or owns the loopback callback. Their built-in `reqwest` adapter targets 0.12; `oauth2-reqwest` 0.1.0-alpha.3 adapts `oauth2` to `reqwest` 0.13 but is pre-release. The OAuth 2 crate warns against following redirects in its token HTTP client because that can enable SSRF. Huskarl 0.11.10 implements authorization-code with PKCE, state, OIDC nonce, and issuer/audience checks. Its `huskarl-reqwest` 0.9.3 adapter uses `reqwest` 0.13. | ADR 0003 selects Huskarl 0.11.10 with default features disabled for authorization code with PKCE, state, callback, and refresh. Production requests no OIDC scopes and consumes no ID tokens. Use `huskarl-reqwest` 0.9.3 with redirects disabled, `reqwest` 0.13 for Graph, and `keyring` 4.2's v1 Linux Secret Service backend. The browser launcher uses `webbrowser` 1.2.6 or later with `hardened`. Synthetic provider tests cover the account flow and profile mapping. Issue #5 is closed after the local Linux gate; tenant evidence remains open in issue #3. |
| System-browser launch | `webbrowser` 1.2.6 (MIT OR Apache-2.0, Rust 1.85; released 2026-10-22) supports the default browser on Windows, macOS, and Linux. Its `hardened` feature rejects non-HTTP(S) URLs. RustSec RUSTSEC-2026-0257 affects versions through 1.2.1; 1.2.2 and later fix the Unix `BROWSER` argument-injection issue. | Selected as a Linux runtime dependency. Synthetic tests verify HTTPS URL handling and reject `file:` URLs without launching a desktop browser. Default-browser and Entra behavior remain open in issue #3. Defer Windows and macOS validation. |
| Account-operation cancellation | `tokio-util` 0.7.20 (MIT, Rust 1.85) is maintained in the Tokio repository; its 2026-10-09 release fixes cancellation-token descendant notification ordering. `CancellationToken` supplies shared, asynchronously awaitable cancellation. The crate has no native build requirements and is portable Rust; only Linux is currently in scope. Its default feature set is empty, and the application uses the workspace's existing `rt` feature selection. The application adds a direct edge to a crate already used by the Microsoft provider, so this does not add another lockfile package. The current lockfile RustSec result is recorded in the candidate review below after the Linux gate. | Use `CancellationToken` for notification and an application-owned atomic phase for the product-specific cancellation-versus-credential-commit ordering. The phase is not delegated to the dependency. Compared with a custom `Future` and `Waker`, this reuses maintained cancellation signaling already present in the workspace. Compared with `futures_util::future::select`, it provides a cloneable shared cancellation handle that crosses the application/provider boundary. No adequate `std` cancellation primitive exists. Synthetic tests cover both winning orders and preserved credentials. Recheck the pinned crate on each dependency update. See [Tokio `tokio-util` 0.7.20](https://github.com/tokio-rs/tokio/tree/tokio-util-0.7.20), its [manifest](https://github.com/tokio-rs/tokio/blob/tokio-util-0.7.20/tokio-util/Cargo.toml), and [cancellation-token API](https://docs.rs/tokio-util/0.7.20/tokio_util/sync/struct.CancellationToken.html). |
| Loopback callback HTTP server | `axum` 0.8.9 (MIT, Rust 1.85; released 2026-04-14) provides maintained HTTP routing and a `with_graceful_shutdown` server API. Its default features include Tokio, Hyper, forms, JSON, and tracing. A callback listener can disable defaults and enable only the required server and query features. `tiny_http` 0.12.0 has not released since 2022-10-06. | Huskarl's authorization-code loopback feature accepts a caller-owned Tokio listener, parses the callback, and releases the listener when the future is cancelled. The desktop app requires a registered nonzero port on `127.0.0.1` under ADR 0031. It does not need a separate callback server crate. |
| Huskarl OAuth desktop flow | `huskarl` 0.11.10 is MIT OR Apache-2.0 and requires Rust 1.92. Its loopback feature accepts a caller-owned Tokio listener and awaits the callback inline. It does not launch the system browser; `webbrowser` remains a separate candidate. The app must own and drop the listener when it cancels sign-in. The tagged CI workflow runs on Ubuntu only. The default verifier feature adds `huskarl-crypto-native` 0.11.3, which pins `rsa` 0.10.0-rc.19 and includes private RSA signing code. RustSec lists no patched RSA release and names rc.18 as affected, so verify rc.19 before adoption. Huskarl exposes `azp` but has a test that accepts a mismatching value. Disabling its default features allows a custom verifier platform. `jsonwebtoken` 11.1.0 is MIT-licensed, supports Rust 1.88, and can verify JWK-backed RS256 with `aws_lc_rs` while omitting its optional RustCrypto `rsa` dependency. AWS-LC needs a C/C++ compiler, and no adapter to Huskarl's verifier API has been proved. | The Linux provider uses Huskarl for authorization code with PKCE, state, token exchange, callback handling, and refresh. Default OIDC verifier features stay disabled because the product flow does not request OIDC scopes or consume ID tokens. Synthetic provider tests use local fixtures and do not contact Entra. Keep issue #3 open for tenant evidence. |
| Other turnkey OAuth loopback helpers | `loopauth` 0.5.1 (MIT OR Apache-2.0) provides PKCE, a loopback callback, token exchange, refresh, and JWKS validation. Its manifest does not declare an MSRV. Its repository last changed on 2026-08-28, and its visible CI runs on Ubuntu only. The API targets CLI token acquisition and leaves app identity and session handling to the caller. It has no public cancellation handle; `run_authorization_flow` detaches the callback server task. The manifest pulls in Axum, `reqwest` 0.13, `webbrowser`, and the full Tokio feature set, as well as Rustls and TLS-certificate support. `oauthkit` 0.2.4 is a CLI SDK with authorization-code/PKCE, device-code, API-key, and credential-storage flows. Its current docs do not provide a verified Windows build, a declared MSRV, or a clear platform CI result. `oauth-toolkit` 0.4.2 (MIT OR Apache-2.0, Rust 1.85; published 2026-10-29) binds `127.0.0.1`, creates PKCE and single-use state, and captures one redirect. Its source implements an HTTP request parser and polls for the callback on a blocking thread. Its manifest adds `reqwest` 0.12, Tokio, `jsonwebtoken`, and broad provider features. The latest [RustSec run](https://github.com/WyattAu/oauth-toolkit/actions/runs/41157846278) reports six advisories, including one unsoundness advisory and one other advisory. | No helper is selected. `loopauth` has a useful integrated flow, but its detached server cannot be cancelled by its caller and Windows behavior is unverified. `oauthkit` has the same Windows evidence gap and no checked MSRV. `oauth-toolkit` is active but reimplements HTTP parsing, duplicates the planned `reqwest` line, and has a failing audit. Prefer maintained protocol, browser, and callback libraries. |
| Operating-system credential storage | [`keyring` 4.2.0](https://docs.rs/crate/keyring/4.2.0) is MIT OR Apache-2.0, declares Rust 1.88, and its `v1` API sets, gets, and deletes binary secrets. On Linux it selects the zbus Secret Service store. Its v1 API also has native Windows Credential Manager and macOS Keychain adapters for later milestones. The Linux feature graph adds `zbus-secret-service-keyring-store` and `secret-service` through `zbus`; zbus is already present through GPUI's Linux stack. Keep the `cli` feature disabled. [`keyring-core` 1.0.1](https://docs.rs/crate/keyring-core/1.0.1) is MIT OR Apache-2.0 and declares Rust 1.85. [`oo7` 0.6.0](https://docs.rs/oo7/0.6.0/oo7/) is an MIT-licensed alternative with Secret Service and encrypted file/portal backends. It is async-only and documents Tokio, async-std, or GLib runtimes; its sandbox-focused file backend is not needed by the current unsandboxed GPUI app. | The Linux Microsoft provider uses `keyring` 4.2.0 with `v1` and without `cli` to store its refresh token in Secret Service. Synthetic provider tests use an in-memory credential store. The separate storage proof covers Fedora GNOME Keyring with synthetic credentials. No real mailbox data or credentials are used. |
| Local encrypted mail cache | `rusqlite` 0.41.1 is the latest release as of 2026-10-31. It is MIT-licensed, declares Rust 1.89, and exposes bundled SQLCipher features. `rusqlite` 0.40.2 is the latest 0.40 patch and is the compatible line for the reviewed migration and async crates, which require `rusqlite ^0.40.0`. Bundled SQLCipher is BSD-licensed and compiles native C code. The vendored OpenSSL feature adds an OpenSSL source build that needs a C compiler, Perl, `Text::Template`, and make; Windows builds also need MSVC and NASM. `SQLx` 0.9.0 does not document a supported SQLCipher SQLite configuration. `redb` 4.4.0 lacks built-in data-at-rest encryption, and Turso labels encryption experimental. | Production Linux persistence uses `async-sqlite` 0.6.0 with defaults disabled and `bundled-sqlcipher-vendored-openssl`, `rusqlite_migration` 2.6.0, and Linux `keyring` 4.2.0 `v1` for Secret Service keys. The async wrapper is MIT, declares Rust 1.95, and provides one bounded background-worker connection; the migration crate is Apache-2.0, declares Rust 1.95, and applies migrations atomically. Synthetic persistence tests cover encrypted records, page/cursor transactions, restart checkpoints, body caching, and cache-file removal. No real mailbox content is used. |
| Calendar date/time and time-zone conversion | Graph `dateTimeTimeZone` carries a local wall-clock value plus a zone identifier. Graph accepts Windows time-zone IDs and additional calendar API IDs. `jiff` 0.2.38 (MIT OR Unlicense, MSRV 1.70, pre-1.0) provides DST-aware datetime types and IANA zone rules; its default features read system zoneinfo on Unix and bundle tzdb for platforms such as Windows. Jiff's Windows lookup maps the machine's current Windows zone through CLDR; it does not expose a mapper for arbitrary Graph zone IDs. `chrono-tz` 0.10.4 (MIT OR Apache-2.0) provides IANA zones, not Windows IDs. `windows-timezones` 0.5.1 maps CLDR Windows names to a default IANA tzdb ID and offers chrono-tz 0.10 integration, but its published API documents a default mapping rather than a territory-aware lookup. `localzone` 0.3.1 has a territory argument for Windows-to-IANA lookup; verify its current maintenance and package metadata before considering it. `time-tz` 2.0.0 advertises closest-IANA matching; its 3.0 line is pre-release. ICU4X 2.3.1 offers Windows-ID mapping with a region, but returns a CLDR/BCP-47 identity and does not ship time-zone offset data. | No dependency is selected. Jiff is the leading datetime candidate, but Graph's arbitrary Windows identifiers still need a trustworthy mapping. Before M3, compare Jiff plus a maintained mapper against `chrono-tz` integrations and prove Windows IDs, additional IANA IDs, unknown IDs, regional ambiguity, daylight-saving gaps/overlaps, and recurring-event behavior with synthetic fixtures. Review the exact published manifests, release activity, security history, feature graph, and binary/build cost for the final pair. Preserve Graph's original zone identifier; do not write a custom mapping unless this review finds no maintained library that meets the requirements. |
| RFC 5322 and MIME parsing | `mail-parser` 1.0.0 handles nested messages and MIME parts. Its current MSRV is Rust 1.98, above this workspace's Rust 1.95 declaration. | Defer unless the app consumes raw MIME. Microsoft Graph message JSON already returns body content. If raw MIME becomes a requirement, compare it with `mailparse` and decide whether the MSRV change is justified. |
| Untrusted HTML sanitization | `ammonia` 4.2.4 (MIT OR Apache-2.0, Rust 1.85) uses `html5ever` 0.40, `cssparser` 0.38, `maplit` 1, and `url` 2. It fits the workspace MSRV of Rust 1.95. RustSec records historical advisories; 4.2.4 includes the fix for RUSTSEC-2026-0373. `htmlsanitizer` 0.2.0 (MIT) is not a full HTML parser, does not state its MSRV in the reviewed metadata, and has a shorter release history. | ADR 0007 accepts Ammonia plus native GPUI `TextView::html` for Linux. The application sanitizes at display time and keeps raw bodies encrypted. `html2text` 0.17.4 provides the bounded oversized-body fallback. Synthetic application and headless GPUI tests cover active and remote content, safe links, image resolution, selection, copy, and truncation. Real-window layout, operating-system clipboard, and browser launch remain open under issue #9. |

### Test-only storage proof dependencies

The dependencies declared in `tools/storage-proof/` are development-only. The package has no application API and does not add persistence code to the desktop runtime. The Microsoft provider separately uses `keyring` for refresh-token storage.

| Crate | Evidence and use |
| --- | --- |
| `getrandom` 0.4.3 | MIT OR Apache-2.0, Rust 1.85, cross-platform system randomness, latest release 2026-06-17. Use its `fill` API for database keys and unique Windows test credential names; no general-purpose RNG is needed. |
| `zeroize` 1.9.1 | Apache-2.0 OR MIT, Rust 1.85, pure Rust and portable. Enable only `alloc`; use `Zeroizing` for raw key arrays, credential byte buffers, and SQLCipher key strings. |
| `hex` 0.4.3 | MIT OR Apache-2.0, no declared MSRV, no runtime dependencies. Use its standard encoding implementation to form SQLCipher's documented raw-key literal, wrapping both temporary and final strings in `Zeroizing`. |
| `smol` 2.0.2 | Apache-2.0 OR MIT, Rust 1.63. Use `block_on` to exercise `async-sqlite` with the same executor family used by GPUI Kit; async-sqlite also documents tests with Tokio. |
| `tempfile` 3.27.0 | MIT OR Apache-2.0, Rust 1.63, maintained cross-platform test utility. Use `TempDir` for automatic cleanup of synthetic database and backup files. |
| `keyring` 4.2.0 | MIT OR Apache-2.0, Rust 1.88, within the workspace's Rust 1.95 MSRV. GitHub lists v4.2.0 as its latest release and 47 commits to `main` since that release. Its `v1` API provides synchronous binary-secret operations and selects the zbus Secret Service backend on Linux. Keep those synchronous calls off the GPUI UI thread. The `cli` feature is disabled, avoiding unrelated credential-store adapters. The Linux backend adds Rust dependencies but no new native build tools; it reuses zbus 5.19.0 already present through GPUI. The current lockfile audit found no vulnerable `keyring` version and reports the unmaintained zbus helper crates documented above. The proof uses random synthetic credential names and removes credentials on completion. |

Local Linux proof passed with `OPENSSL_NO_VENDOR=1` because this Fedora image lacks `FindBin.pm`; that run used system OpenSSL. Linux CI installed the vendored OpenSSL build requirements and passed without that override. Windows x64 CI also passed with vendored OpenSSL. The required Fedora modules are `perl-FindBin` and `perl-Text-Template`; Ubuntu installs `perl` and `libtext-template-perl`. Windows needs Perl, `Text::Template`, MSVC, and NASM for the vendored OpenSSL build.

The Windows runner image lists Perl but does not list NASM or `Text::Template`, so CI installs these explicitly. `shogo82148/actions-setup-perl` 1.44.2 supports Windows Server 2025, uses Strawberry Perl, and installs the required CPAN module. `ilammy/setup-nasm` 1.5.2 installs the official NASM binary; CI pins NASM 2.16.01. Both actions are MIT-licensed build tools and are not application dependencies. The hosted Windows setup passed in PR #15.

### Native HTML-to-text fallback — 2026-11-01

Use `html2text` 0.17.4 for the native reader's bounded plain-text fallback. Its upstream repository is [`jugglerchris/rust-html2text`](https://github.com/jugglerchris/rust-html2text). Releases 0.17.2 through 0.17.4 fix malformed tables, a nested-render-tree stack overflow, and an infinite loop on malformed CSS. The selected build enables no optional features, so it does not enable document CSS or syntax extensions. The crate is MIT licensed and requires Rust 1.85, below the workspace's 1.95 MSRV. It uses `html5ever` 0.40.1, which Ammonia 4.2.4 already brings in. It adds no second version. Its plain non-decorating configuration does not emit link footnotes. The application feeds it only Ammonia-sanitized markup. It reads at most 1 MiB and keeps at most 64 KiB of converted text before escaping it for native rendering. The 2026-11-01 Linux gate scanned 918 locked crates with no vulnerable or yanked versions. It reported the same eight unmaintained transitive crates already tracked above. No crate-specific RustSec advisory appeared in the reviewed package search. The lockfile audit remains authoritative.

| Candidate | Evidence | Decision |
| --- | --- | --- |
| `html2text` 0.17.4 | [`MIT` package metadata](https://docs.rs/crate/html2text/0.17.4), [manifest](https://docs.rs/crate/html2text/0.17.4/source/Cargo.toml), and [changelog](https://docs.rs/crate/html2text/0.17.4/source/CHANGELOG.md). Rust 1.85; HTML5 parsing; portable Rust implementation; empty default feature set. Current Ammonia dependency already resolves the same `html5ever` 0.40.1 version. | Use in `crates/application` for plain-text conversion of Ammonia output. Enable no optional features. Cap sanitized input at 1 MiB and plain text at 64 KiB so Ammonia-escaped native HTML stays below 512 KiB. |
| `htmd` 0.5.6 | [Package metadata](https://docs.rs/crate/htmd/0.5.6) and [upstream source](https://github.com/letmutex/htmd). Apache-2.0 licensed. Its MSRV is not declared. It converts HTML to Markdown. | Reject for this output contract: Markdown decorations and link syntax are not plaintext and could be reinterpreted as active formatting by a Markdown view. |
| Custom tag stripping | No dependency cost. | Reject: reliable HTML text extraction needs HTML parsing and recovery; a handwritten stripper would mishandle malformed markup and entity decoding. |

The combined M2 candidate's documented Linux gate passed locally on 2026-11-01. The RustSec result is recorded above. No GUI or live Graph request was made. No parser is added to the desktop crate, and no WebView is added.

## Current constraints

- The Microsoft provider uses Graph, OAuth, browser, HTTP, and Secret Service libraries. Application workflows keep provider-neutral account and mail types. Graph mail access is read-only in M2.
- ADR 0005 selects SQLCipher-backed SQLite. Production Linux persistence uses the reviewed async wrapper and migration crate for encrypted mail records, page/cursor transactions, and account-scoped cache removal. The Windows x64 evidence is historical; current platform work targets Linux.
- Linux is the sole current development and validation target. Windows and macOS implementation work and validation are deferred to later roadmap milestones.
- No dedicated Microsoft 365 test tenant or public app registration is available. Synthetic Graph and OAuth tests do not establish live Entra or mailbox behavior.
- Both ignored Linux credential lifecycle tests passed in Fedora GNOME Keyring on 2026-10-31. The service was unlocked; the proof covers unavailable service and synthetic locked-store errors separately.
- Keep OAuth tokens out of GPUI views, logs, local mail records, and message rendering.
- Keep the main application UI in GPUI. Use the system browser for sign-in. The accepted M2 renderer uses native GPUI `TextView::html`; a WebView needs its own security decision.
- Do not add a candidate just because it appears in this list. Recheck its current release and test the boundary it will own.

## Sources

- [`cargo-deny` 0.20.2 release](https://github.com/EmbarkStudios/cargo-deny/releases/tag/0.20.2)
- [`cargo-deny` package manifest](https://github.com/EmbarkStudios/cargo-deny/blob/main/Cargo.toml)
- [`cargo-deny` bans configuration](https://embarkstudios.github.io/cargo-deny/checks/bans/cfg.html)
- [`cargo-deny` package specifications](https://embarkstudios.github.io/cargo-deny/checks/cfg.html)
- [Rust tracking issue #133508 for slice-to-array conversions](https://github.com/rust-lang/rust/issues/133508)
- [Rust stabilization PR #147540 for `[T]::as_array`](https://github.com/rust-lang/rust/pull/147540)
- [Rust tracking issue #136873 for `cold_path`](https://github.com/rust-lang/rust/issues/136873)
- [Rust 1.93.0 release](https://blog.rust-lang.org/2026/01/22/Rust-1.93.0/)
- [Rust 1.95.0 release](https://blog.rust-lang.org/2026/04/16/Rust-1.95.0/)
- [Microsoft Graph SDK overview](https://learn.microsoft.com/en-us/graph/sdks/sdks-overview)
- [`graph-rs-sdk` 3.0.1 on docs.rs](https://docs.rs/crate/graph-rs-sdk/3.0.1)
- [`graph-rs-sdk` releases](https://github.com/sreeise/graph-rs-sdk/releases)
- [Kiota supported languages and maturity](https://learn.microsoft.com/en-us/openapi/kiota/support)
- [`io-msgraph` 0.6.0 documentation](https://docs.rs/io-msgraph/0.6.0/io_msgraph/)
- [`io-msgraph` source repository](https://github.com/pimalaya/io-msgraph)
- [Microsoft guidance for Graph use without a supported SDK](https://github.com/MicrosoftDocs/microsoft-cloud/blob/main/docs/dev/dev-proxy/concepts/why-use-microsoft-graph-sdk.md)
- [`reqwest` documentation](https://docs.rs/reqwest/latest/reqwest/)
- [`reqwest` 0.13.5 release](https://github.com/seanmonstar/reqwest/releases/tag/v0.13.5)
- [`reqwest` retry behavior](https://docs.rs/reqwest/0.13.5/reqwest/retry/)
- [`reqwest-retry-after` 0.2.1 documentation](https://docs.rs/reqwest-retry-after/0.2.1/reqwest_retry_after/)
- [`io-msgraph` 0.6.0 changelog](https://docs.rs/crate/io-msgraph/0.6.0/source/CHANGELOG.md)
- [`io-msgraph` 0.6.0 manifest](https://docs.rs/crate/io-msgraph/0.6.0/source/Cargo.toml.orig)
- [`io-msgraph` Graph error handling](https://raw.githubusercontent.com/pimalaya/io-msgraph/v0.6.0/src/v1/send.rs)
- [`graph-rs-sdk` 3.0.1 documentation and dependency list](https://docs.rs/crate/graph-rs-sdk/3.0.1)
- [`isahc` 2.0.2 documentation](https://docs.rs/isahc/2.0.2/isahc/)
- [`ureq` 3.5.0 documentation](https://docs.rs/ureq/3.5.0/ureq/)
- [Microsoft Graph throttling guidance](https://learn.microsoft.com/en-us/graph/throttling)
- [Microsoft Graph immutable Outlook identifiers](https://learn.microsoft.com/en-us/graph/outlook-immutable-id)
- [RustSec RUSTSEC-2026-0346 for rustls](https://rustsec.org/advisories/RUSTSEC-2026-0346)
- [RustSec RUSTSEC-2026-0347 for rustls](https://rustsec.org/advisories/RUSTSEC-2026-0347)
- [RustSec RUSTSEC-2026-0348 for rustls](https://rustsec.org/advisories/RUSTSEC-2026-0348)
- [RustSec RUSTSEC-2026-0349 for rustls](https://rustsec.org/advisories/RUSTSEC-2026-0349)
- [`serde` documentation](https://serde.rs/)
- [`uuid` 1.29.0 documentation](https://docs.rs/uuid/1.29.0/uuid/)
- [`uuid` upstream manifest](https://github.com/uuid-rs/uuid/blob/main/Cargo.toml)
- [`oauth2` 5.0.0 documentation](https://docs.rs/oauth2/5.0.0/oauth2/)
- [`oauth2-reqwest` 0.1.0-alpha.3 documentation](https://docs.rs/oauth2-reqwest/0.1.0-alpha.3/oauth2_reqwest/)
- [`openidconnect` 4.0.1 documentation](https://docs.rs/openidconnect/4.0.1/openidconnect/)
- [`openidconnect` 4.0.1 manifest](https://docs.rs/crate/openidconnect/4.0.1/source/Cargo.toml.orig)
- [`huskarl` 0.11.10 documentation and package metadata](https://docs.rs/crate/huskarl/0.11.10)
- [`huskarl` 0.11.10 package manifest](https://github.com/huskarl-rs/huskarl/blob/huskarl-v0.11.10/huskarl/Cargo.toml)
- [`huskarl-crypto-native` 0.11.3 package manifest](https://github.com/huskarl-rs/huskarl/blob/huskarl-v0.11.10/huskarl-crypto-native/Cargo.toml)
- [`huskarl` loopback callback source](https://github.com/huskarl-rs/huskarl/blob/huskarl-v0.11.10/huskarl/src/grant/authorization_code/loopback.rs)
- [`huskarl` authorization-code and OIDC flow source](https://github.com/huskarl-rs/huskarl/blob/huskarl-v0.11.10/huskarl/src/grant/authorization_code/flow.rs)
- [`huskarl` ID-token validation and `azp` behavior](https://github.com/huskarl-rs/huskarl/blob/huskarl-v0.11.10/huskarl/src/token/id_token.rs#L1451-L1468)
- [`huskarl` 0.11.10 CI workflow](https://github.com/huskarl-rs/huskarl/blob/huskarl-v0.11.10/.github/workflows/ci.yml)
- [`huskarl-reqwest` 0.9.3 documentation and package metadata](https://docs.rs/crate/huskarl-reqwest/0.9.3)
- [`huskarl-reqwest` 0.9.3 source](https://docs.rs/crate/huskarl-reqwest/0.9.3/source/src/lib.rs)
- [`huskarl-core` custom JWS verifier guide](https://github.com/huskarl-rs/huskarl/blob/huskarl-v0.11.10/huskarl-core/docs/guide/configuring_jwt_verification.md)
- [RustSec RUSTSEC-2023-0071 for RSA](https://rustsec.org/advisories/RUSTSEC-2023-0071.html)
- [`jsonwebtoken` 11.1.0 documentation and dependency list](https://docs.rs/crate/jsonwebtoken/11.1.0)
- [`jsonwebtoken` 11.1.0 package manifest](https://docs.rs/crate/jsonwebtoken/11.1.0/source/Cargo.toml.orig)
- [`aws-lc-rs` build requirements](https://docs.rs/aws-lc-rs/latest/aws_lc_rs/)
- [`webbrowser` documentation](https://docs.rs/webbrowser/latest/webbrowser/)
- [`webbrowser` 1.2.5 release notes](https://github.com/amodm/webbrowser-rs/releases/tag/v1.2.5)
- [`webbrowser` 1.2.5 manifest](https://github.com/amodm/webbrowser-rs/blob/v1.2.5/Cargo.toml)
- [RustSec RUSTSEC-2026-0257 for `webbrowser`](https://rustsec.org/advisories/RUSTSEC-2026-0257.html)
- [`axum` 0.8.9 documentation](https://docs.rs/axum/0.8.9/axum/)
- [`axum` 0.8.9 manifest](https://github.com/tokio-rs/axum/blob/axum-v0.8.9/axum/Cargo.toml)
- [`axum::serve::Serve::with_graceful_shutdown`](https://docs.rs/axum/0.8.9/axum/serve/struct.Serve.html#method.with_graceful_shutdown)
- [`tiny_http` 0.12.0 release history](https://docs.rs/crate/tiny_http/0.12.0)
- [`loopauth` 0.5.1 documentation](https://docs.rs/loopauth/0.5.1/loopauth/)
- [`loopauth` manifest](https://github.com/joshuaclayton/loopauth/blob/main/Cargo.toml)
- [`loopauth` CI workflow](https://github.com/joshuaclayton/loopauth/blob/main/.github/workflows/ci.yml)
- [`loopauth` callback server source](https://github.com/joshuaclayton/loopauth/blob/main/src/server.rs)
- [`loopauth` authorization-flow source](https://github.com/joshuaclayton/loopauth/blob/main/src/builder.rs)
- [`oauthkit` 0.2.4 documentation](https://docs.rs/oauthkit/0.2.4/oauthkit/)
- [`oauth-toolkit` 0.4.2 documentation](https://docs.rs/oauth-toolkit/latest/oauth_toolkit/)
- [`oauth-toolkit` 0.4.2 manifest](https://github.com/WyattAu/oauth-toolkit/blob/8a9a3dd4f8a2d0811049d887af30c9ba6e1c1794/Cargo.toml)
- [`oauth-toolkit` loopback source](https://github.com/WyattAu/oauth-toolkit/blob/8a9a3dd4f8a2d0811049d887af30c9ba6e1c1794/src/loopback.rs)
- [Microsoft MSAL overview and supported libraries](https://learn.microsoft.com/en-us/entra/identity-platform/msal-overview)
- [Microsoft Azure Identity for Rust](https://docs.rs/azure_identity/1.0.0/azure_identity/)
- [Microsoft guidance for apps that cannot use MSAL](https://learn.microsoft.com/en-us/entra/architecture/authenticate-applications-and-users)
- [Microsoft Windows Account Manager (WAM) desktop flow](https://learn.microsoft.com/en-us/entra/identity-platform/scenario-desktop-acquire-token-wam)
- [`keyring` 4.2.0 documentation](https://docs.rs/crate/keyring/4.2.0)
- [`keyring` 4.2.0 release notes](https://github.com/open-source-cooperative/keyring-rs/releases)
- [`keyring-core` 1.0.1 documentation](https://docs.rs/crate/keyring-core/1.0.1)
- [`windows-native-keyring-store` 1.1.0 documentation](https://docs.rs/crate/windows-native-keyring-store/1.1.0)
- [`windows-native-keyring-store` 1.1.0 manifest](https://docs.rs/crate/windows-native-keyring-store/1.1.0/source/Cargo.toml.orig)
- [`getrandom` 0.4.3 documentation and release notes](https://docs.rs/crate/getrandom/0.4.3)
- [`zeroize` 1.9.1 documentation](https://docs.rs/crate/zeroize/1.9.1)
- [`hex` 0.4.3 documentation and manifest](https://docs.rs/crate/hex/0.4.3)
- [`smol` 2.0.2 documentation and manifest](https://docs.rs/crate/smol/2.0.2)
- [`tempfile` 3.27.0 documentation and manifest](https://docs.rs/crate/tempfile/3.27.0)
- [`xdg` 3.0.0 API documentation](https://docs.rs/xdg/3.0.0/xdg/struct.BaseDirectories.html)
- [`directories` 6.0.0 documentation](https://docs.rs/crate/directories/6.0.0)
- [`directories-rs` upstream move notice](https://github.com/dirs-dev/directories-rs)
- [OpenSSL build prerequisites](https://github.com/openssl/openssl/blob/master/INSTALL.md)
- [OpenSSL Windows build prerequisites](https://github.com/openssl/openssl/blob/master/NOTES-WINDOWS.md)
- [Fedora 44 `perl-FindBin` package](https://packages.fedoraproject.org/pkgs/perl/perl-FindBin/)
- [Fedora 44 `perl-Text-Template` package](https://packages.fedoraproject.org/pkgs/perl-Text-Template/perl-Text-Template/)
- [`rusqlite` 0.41.1 documentation](https://docs.rs/crate/rusqlite/0.41.1)
- [`rusqlite` SQLCipher features and native build notes](https://docs.rs/crate/rusqlite/0.41.1/source/README.md)
- [`rusqlite` 0.40.2 documentation](https://docs.rs/crate/rusqlite/0.40.2)
- [`rusqlite` 0.40.2 SQLCipher features and native build notes](https://docs.rs/crate/rusqlite/0.40.2/source/README.md)
- [`rusqlite_migration` 2.6.0 documentation and dependency list](https://docs.rs/crate/rusqlite_migration/2.6.0) (Rust 1.95, rusqlite 0.40)
- [`rusqlite_migration` 2.6.0 manifest](https://docs.rs/crate/rusqlite_migration/2.6.0/source/Cargo.toml.orig)
- [`rusqlite_migration` release history](https://github.com/cljoly/rusqlite_migration/releases)
- [`refinery` releases](https://github.com/rust-db/refinery/releases) (0.10.0 supports rusqlite 0.40.x and Rust 1.92)
- [`refinery` 0.10.0 manifest](https://docs.rs/crate/refinery/0.10.0/source/Cargo.toml)
- [`async-rusqlite` 0.6.0 documentation](https://docs.rs/crate/async-rusqlite/0.6.0/source/)
- [`async-rusqlite` 0.6.0 manifest](https://docs.rs/crate/async-rusqlite/0.6.0/source/Cargo.toml.orig)
- [`async-sqlite` 0.6.0 documentation and manifest](https://docs.rs/crate/async-sqlite/0.6.0)
- [`async-sqlite` 0.6.0 manifest](https://docs.rs/crate/async-sqlite/0.6.0/source/Cargo.toml.orig)
- [`refinery` 0.10.0 release notes](https://github.com/rust-db/refinery/releases)
- [RustSec rusqlite advisories](https://rustsec.org/packages/rusqlite.html)
- [`SQLx` SQLite driver documentation](https://docs.rs/sqlx/0.9.0/sqlx/sqlite/)
- [`redb` documentation](https://docs.rs/redb/4.4.0/redb/)
- [Turso manual and its encryption status](https://github.com/tursodatabase/turso/blob/main/docs/manual.md)
- [`rusqlite` Windows SQLCipher report](https://github.com/rusqlite/rusqlite/issues/966) (community report; Windows x64 and ARM64 results differ)
- [OpenSSL crate build requirements](https://docs.rs/openssl/latest/openssl/#vendored)
- [OpenSSL Windows build requirements](https://github.com/openssl/openssl/blob/master/NOTES-WINDOWS.md)
- [GitHub Actions Windows Server 2025 runner image inventory](https://github.com/actions/runner-images/blob/main/images/windows/Windows2025-Readme.md)
- [`actions-setup-perl` 1.44.2](https://github.com/shogo82148/actions-setup-perl/tree/v1.44.2)
- [`setup-nasm` 1.5.2](https://github.com/ilammy/setup-nasm/tree/v1.5.2)
- [`mail-parser` 1.0.0 documentation](https://docs.rs/crate/mail-parser/1.0.0)
- [`ammonia` 4.2.4 package metadata and documentation](https://docs.rs/crate/ammonia/4.2.4)
- [`ammonia` changelog](https://github.com/rust-ammonia/ammonia/blob/master/CHANGELOG.md)
- [RustSec advisories for Ammonia](https://rustsec.org/packages/ammonia.html)
- [RUSTSEC-2024-0384: `instant` is unmaintained](https://rustsec.org/advisories/RUSTSEC-2024-0384.html)
- [RUSTSEC-2024-0436: `paste` is unmaintained](https://rustsec.org/advisories/RUSTSEC-2024-0436.html)
- [RUSTSEC-2025-0134: `rustls-pemfile` is unmaintained](https://rustsec.org/advisories/RUSTSEC-2025-0134.html)
- [RUSTSEC-2026-0206: `rustybuzz` is unmaintained](https://rustsec.org/advisories/RUSTSEC-2026-0206.html)
- [RUSTSEC-2026-0192: `ttf-parser` is unmaintained](https://rustsec.org/advisories/RUSTSEC-2026-0192.html)
- [RUSTSEC-2026-0371: `zbus_names` is unmaintained](https://rustsec.org/advisories/RUSTSEC-2026-0371.html)
- [RUSTSEC-2026-0370: `zvariant` is unmaintained](https://rustsec.org/advisories/RUSTSEC-2026-0370.html)
- [RUSTSEC-2026-0372: `zvariant_derive` is unmaintained](https://rustsec.org/advisories/RUSTSEC-2026-0372.html)
- [RUSTSEC-2026-0373: CSS URL-filtering issue patched in ammonia 4.2.4](https://rustsec.org/advisories/RUSTSEC-2026-0373.html)
- [`htmlsanitizer` 0.2.0 package metadata and documentation](https://docs.rs/crate/htmlsanitizer/0.2.0)
- [GPUI Kit TextView documentation](https://gpui-kit.com/base/text-view/)
- [GPUI Kit WebView guide and limitations](https://gpui-kit.com/docs/webview/)
- [Microsoft Graph dateTimeTimeZone resource](https://learn.microsoft.com/en-us/graph/api/resources/datetimetimezone?view=graph-rest-1.0)
- [`jiff` 0.2.38 manifest](https://github.com/BurntSushi/jiff/blob/0.2.38/crates/jiff/Cargo.toml)
- [`jiff` 0.2.38 changelog](https://github.com/BurntSushi/jiff/blob/0.2.38/CHANGELOG.md)
- [`jiff` time-zone and platform documentation](https://docs.rs/jiff/0.2.38/jiff/tz/)
- [`chrono-tz` 0.10.4 package documentation](https://docs.rs/crate/chrono-tz/0.10.4)
- [`chrono-tz` upstream manifest](https://github.com/chronotope/chrono-tz/blob/main/chrono-tz/Cargo.toml)
- [`windows-timezones` 0.5.1 documentation](https://docs.rs/windows-timezones/0.5.1/windows_timezones/)
- [`localzone` 0.3.1 Windows-to-IANA function](https://docs.rs/localzone/0.3.1/localzone/fn.win_zone_to_iana.html)
- [`time-tz` package documentation and release history](https://docs.rs/crate/time-tz/latest)
- [`time-tz` upstream repository and manifest](https://github.com/Yuri6037/time-tz)
- [ICU4X `TimeZone` Windows-ID mapping](https://docs.rs/icu/latest/icu/time/zone/struct.TimeZone.html)
- [ICU4X time-zone API limits](https://docs.rs/icu/latest/icu/time/zone/index.html)
