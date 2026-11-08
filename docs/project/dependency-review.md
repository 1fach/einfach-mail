# Rust dependency review

Last reviewed: 2026-11-08. These notes record dependency evidence and adopted choices. Recheck versions and project fit before adding or upgrading a crate.

## Review rules

When a need comes up, search for a maintained Rust library before writing general-purpose code. Use a suitable library when it meets the product, platform, and security requirements; write custom code only when the review shows that no good library fits, and record why. Before adding a direct dependency, record its purpose, current version, license, supported platforms, MSRV, maintenance activity, security history, feature graph, and API fit. Check transitive dependencies and build requirements. Compare at least one alternative when the choice affects authentication, storage, network access, or untrusted content. Add an integration proof before accepting a security-sensitive dependency.

Update this file when research changes. Record lasting decisions in an ADR. Keep dependency tasks in GitHub issues after a remote is configured.

Linux is the only active development target. Evaluate current candidates for Linux; record Windows and macOS gaps for their later support milestones.

## Development tools

| Need | Tool and evidence | Project choice |
| --- | --- | --- |
| Rust lockfile advisory audit | [`cargo-audit` 0.22.2](https://github.com/rustsec/rustsec/blob/main/cargo-audit/Cargo.toml) is the RustSec tool for auditing `Cargo.lock` against the [RustSec Advisory Database](https://github.com/RustSec/advisory-db). It is actively maintained, licensed MIT OR Apache-2.0, and declares Rust 1.88 as its MSRV. Its default `binary-scanning` feature is not needed for this lockfile check. | Pin 0.22.2 in a separate read-only CI workflow and install with `--no-default-features`; run `cargo audit --deny yanked` so yanked locked versions fail CI. The official [`rustsec/audit-check` action](https://github.com/rustsec/audit-check) takes a GitHub token and documents check/issue write permissions; direct CLI execution keeps this workflow at `contents: read`. `cargo-deny` is used separately for exact package bans; keep RustSec advisory auditing in `cargo-audit`. |
| Optional WebView dependency ban | [`cargo-deny` 0.20.2](https://github.com/EmbarkStudios/cargo-deny/blob/main/Cargo.toml) is actively maintained, MIT OR Apache-2.0, and declares Rust 1.88 as its MSRV. Its `bans` check supports exact package bans with reasons. | Pin 0.20.2 in the workspace dependency policy CI job and run `cargo deny check bans` for the active Linux targets with all workspace features. `deny.toml` bans `wry` and `gpui-webview` until the optional body-only WebView passes the security review in ADR 0007. Use `cargo-audit` for RustSec advisory and yanked-version policy; do not configure cargo-deny's unrelated license, source, or duplicate-version checks without a reviewed policy. |
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

These crates are transitive lockfile entries, not direct workspace dependencies. The first audit run used a temporary `CARGO_HOME` and could not locate `cargo` because `~/.cargo/bin` is not on this shell's `PATH`; it therefore could not update the crates.io index. A follow-up run set `CARGO` to the installed Cargo binary and completed `cargo audit --deny yanked`, updating the index successfully. It emitted no vulnerable-version or yanked-version warnings. No advisory was ignored. The RustSec workflow passed against the branch's committed lockfile in [PR #13 run 41155947067](https://github.com/1fach/einfach-mail/actions/runs/41155947067). Issue #10 was still open at the time of this scan and closed on 2026-10-31.

### M1 candidate lockfile audit — 2026-11-01

`cargo audit --db /tmp/hey-mail-rustsec-advisory-db --deny yanked` loaded 1,395 advisories and scanned 889 locked crate dependencies. It found no known vulnerable versions or yanked crates and reported the same eight upstream unmaintained transitive crates listed above. It reported no advisory for `tokio-util` 0.7.20. No advisory was ignored. The temporary advisory database was removed after the audit.

### M2 local candidate lockfile audit — 2026-11-01

`cargo audit --db /tmp/m2-read-only-mail-rustsec-db --deny yanked` loaded 1,395 advisories and scanned 916 locked crate dependencies. It found no known vulnerable versions or yanked crates and reported the same eight upstream unmaintained transitive crates listed above. No advisory was ignored. The temporary advisory database is kept outside the repository and removed after the local review.

### Yanked executor lockfile follow-up — 2026-11-03

The first RustSec workflow run for PR #47 loaded 1,403 advisories and rejected the locked `async-executor` 1.14.0 under `cargo audit --deny yanked` ([run 41661798059](https://github.com/1fach/einfach-mail/actions/runs/41661798059)). The crates.io index marks 1.14.0 yanked. The upstream [`async-executor` changelog](https://github.com/smol-rs/async-executor/blob/master/CHANGELOG.md) says 1.14.1 fixes a use-after-free when a waker fires during `Executor::drop`, introduced in 1.14.0. The transitive paths include GPUI Kit, `smol`, and Linux `zbus` integrations. Update only the locked patch release to 1.14.1; do not replace or directly depend on the executor.

The follow-up lockfile audit loaded the same 1,403 advisories and scanned 916 dependencies. It found no vulnerable or yanked versions and reported the eight unmaintained transitive crates already triaged above. `cargo deny check bans`, the Rust 1.95 workspace/all-target check, and the full workspace test suite passed locally with the new lockfile. The provider token and persistence cache-key lifecycle tests remain ignored because they require a desktop Secret Service.

### M4 account-removal candidate audit — 2026-11-04

The Issue #43 workspace lockfile audit used cargo-audit 0.22.2 with cargo audit --db /home/frenico/Documents/development/hey-mail/target/rustsec-advisory-db --deny yanked. RustSec loaded 1,403 advisories and scanned 918 locked crate dependencies. It found no vulnerable or yanked versions and reported the same eight upstream unmaintained transitive crates listed below. No advisory was ignored; xdg 3.0.0 has no matching advisory. The Linux workspace tests, Rust 1.95 all-target check, Clippy, rustdoc, formatting, and dependency policy checks passed locally. The desktop process-lock test uses a synthetic child process and does not open a GUI.

### Transitive unmaintained advisory review (2026-11-08)

The local `cargo audit --db /tmp/einfach-dep-review-advisory-db --deny yanked` run loaded 1,422 RustSec advisories and scanned 947 locked dependencies. It found no vulnerable or yanked versions. It reported eight `INFO Unmaintained` advisories. `cargo tree` confirms that seven findings are in the active Linux graph. `rustls-pemfile` appears only in the cross-target lockfile through GPUI's HTTP stack.

GPUI Kit [v0.9.0](https://github.com/longbridge/gpui-kit/releases/tag/v0.9.0) was released on 2026-11-02. Its [tagged workspace manifest](https://github.com/longbridge/gpui-kit/blob/v0.9.0/Cargo.toml) pins the `gpui-pre` crates to 0.3.11. Its [component manifest](https://github.com/longbridge/gpui-kit/blob/v0.9.0/crates/component/Cargo.toml) still depends on `instant` and `paste`.

The [v0.9.0 lockfile](https://github.com/longbridge/gpui-kit/blob/v0.9.0/Cargo.lock) still contains all eight advisory package names. It resolves `instant` 0.1.13, `paste` 1.0.15, `rustls-pemfile` 2.2.0, `rustybuzz` 0.20.1, `ttf-parser` 0.25.1, `zbus_names` 4.3.4, `zvariant` 5.15.0, and `zvariant_derive` 5.15.0. It still uses `resvg` 0.46.0 and the zbus 5 stack. GPUI Kit 0.9.0 therefore does not remove any of the eight findings. This advisory review alone did not justify an upgrade; issue #214 separately evaluated 0.9.0 for its accessibility, text-rendering, virtual-list, and shutdown fixes.

| Crate | Linux dependency path | Replacement and disposition |
| --- | --- | --- |
| `instant` 0.1.13 | `gpui-base` and `gpui-component` | RustSec recommends [`web-time`](https://rustsec.org/advisories/RUSTSEC-2024-0384.html). GPUI Kit 0.9.0 still depends on `instant`; replace it when GPUI Kit changes the owning dependency. |
| `paste` 1.0.15 | `gpui-component` and the `pulp` → `exr` image path | RustSec recommends [`pastey`](https://rustsec.org/advisories/RUSTSEC-2024-0436.html). The latest [pastey release](https://github.com/as1100k/pastey/releases) is 0.2.3; its [manifest](https://github.com/as1100k/pastey/blob/master/Cargo.toml) declares MIT OR Apache-2.0 and Rust 1.54, and its README documents a compatible Cargo alias. The GPUI Kit 0.9.0 component manifest still declares `paste = "1"`; the app would need to maintain a package shim or fork GPUI to redirect that edge. Leave the migration to GPUI Kit. |
| `rustls-pemfile` 2.2.0 | Cross-target only: `gpui-kit-assets` → `gpui-pre-reqwest` 0.12.15 | RustSec recommends the `PemObject` API in [`rustls-pki-types`](https://rustsec.org/advisories/RUSTSEC-2025-0134.html). The crate is absent from the active Linux target graph. The GPUI Kit 0.9.0 lockfile still contains it through GPUI's HTTP stack. Einfach Mail does not parse PEM data, so wait for `gpui-pre-reqwest` to migrate. |
| `rustybuzz` 0.20.1 | `usvg` 0.46.0 → `gpui-pre` renderer | RustSec lists [`harfrust`](https://rustsec.org/advisories/RUSTSEC-2026-0206.html). `resvg` 0.48 moved from `rustybuzz` to `harfrust`; the current `resvg` release is 0.49.0 with Rust 1.85 MSRV. GPUI Kit 0.9.0 still resolves `resvg` 0.46.0, so a compatible GPUI renderer release must carry the update. ([resvg changelog](https://github.com/linebender/resvg/blob/main/CHANGELOG.md)) |
| `ttf-parser` 0.25.1 | `fontdb` → `cosmic-text`, and `usvg` → `gpui-pre` | RustSec lists [`skrifa`](https://rustsec.org/advisories/RUSTSEC-2026-0192.html). `resvg` 0.48 moved to Skrifa, but the active `fontdb` path still depends on `ttf-parser`; replacing only `resvg` would not remove the Linux warning. GPUI Kit 0.9.0 retains `ttf-parser` in both paths. |
| `zbus_names` 4.3.5 | `zbus` 5.19.0, `zbus_xml`, and AT-SPI validation | RustSec says its API moved into [`zbus` 6](https://rustsec.org/advisories/RUSTSEC-2026-0371.html). The GPUI Kit 0.9.0 lockfile still uses `zbus_names` 4 and `zvariant` 5. The current GPUI, portal, accessibility, notification, and Secret Service crates use zbus 5. Upgrade this stack together when compatible releases are available. |
| `zvariant` 5.15.1 | zbus 5, portals, accessibility, and Secret Service | RustSec says its API moved into [`zbus` 6](https://rustsec.org/advisories/RUSTSEC-2026-0370.html). GPUI Kit 0.9.0 still resolves `zvariant` 5. Do not replace one branch of the shared D-Bus type graph by itself. |
| `zvariant_derive` 5.15.1 | `zvariant` 5.15.1 | RustSec says the derive macros moved into [`zbus` 6](https://rustsec.org/advisories/RUSTSEC-2026-0372.html). GPUI Kit 0.9.0 still resolves `zvariant_derive` 5. The migration follows the coordinated zbus update. |

I found no worthwhile app-owned dependency update. Each finding comes from a dependency whose package name or major API is set by GPUI or its Linux integrations. A local shim or fork would make Einfach Mail maintain compatibility code for its UI framework. The remaining findings need coordinated renderer, TLS, and zbus updates from their owners. The audit found no vulnerable or yanked versions, so no local patch or suppression is justified. No ADR is needed for the advisory review itself. Recheck the graph when GPUI Kit or its Linux integration dependencies publish compatible releases. [Issue #206](https://github.com/1fach/einfach-mail/issues/206) records this follow-up; [issue #128](https://github.com/1fach/einfach-mail/issues/128) tracked the initial review.

## Candidates

### GPUI Kit 0.9.0 upgrade review — 2026-11-08

| Candidate | Evidence | Decision |
| --- | --- | --- |
| GPUI Kit 0.9.0 | The [official release](https://github.com/longbridge/gpui-kit/releases/tag/v0.9.0) adds accessibility, menu, table, virtual-list, and text-rendering fixes and fixes a shutdown deadlock. It upgrades the default `gpui-pre` backend to 0.3.11 and the optional GPUI Fast backend to 0.1.13. The release marks public theme structs `#[non_exhaustive]`, changes `Window::text_system()` to return `Rc`, and moves animation timing to the executor clock. The application has no affected theme struct literals, text-system calls, or GPUI animation wall-clock assumptions; `cargo +1.95.0 check --workspace --all-targets --locked` passed. The 0.9.0 package is Apache-2.0. Its package manifest does not declare `rust-version`; the full workspace was checked on the project's Rust 1.95 minimum. The Linux graph excludes GPUI Fast, `gpui-webview`, and Wry. The release retains all eight unmaintained RustSec packages described above. Refreshing the vendored Apache-2.0 component replaces about 4.5 MiB of source and requires maintaining the DatePicker patch; `scripts/check-gpui-component-vendor.sh` verifies the published crate checksum, applies the patch, and compares the result. | Upgrade the workspace to 0.9.0. Its fixes benefit existing accessibility and text paths, and the API changes require no app migration. Keep the accessible DatePicker patch and headless regression test. The known RustSec warnings remain upstream and are not the reason for this upgrade. The headless composer test passed with the patch. |

### Domain validation errors — 2026-11-07

| Candidate | Evidence | Decision |
| --- | --- | --- |
| `thiserror` 2.0.21 | [Upstream documentation](https://docs.rs/crate/thiserror/latest) describes the `Error` derive for unit structs and enums, with explicit `#[source]` fields. The package is MIT OR Apache-2.0, declares Rust 1.77, and uses only the Rust proc-macro implementation crate. Its `std` feature is enabled by default; it adds no native build tools or platform-specific dependencies. Version 2.0.21 is already locked and used directly by application and persistence; the Linux `cargo audit` reports no thiserror advisory. The domain manifest adds a direct edge to the existing version, with no new packages or version changes. | Use the derive for domain validation errors. It preserves their public error types and supports the existing source chain without handwritten `Display` and `Error` implementations. Keep domain messages on their types and variants. |

### Redacted domain Debug formatting — 2026-11-07

| Candidate | Evidence | Decision |
| --- | --- | --- |
| `derive_more` 2.1.1 | [Package metadata and manifest](https://docs.rs/crate/derive_more/2.1.1), [Debug derive documentation](https://docs.rs/derive_more/2.1.1/derive_more/derive.Debug.html), and [upstream changelog](https://docs.rs/crate/derive_more/2.1.1/source/CHANGELOG.md). MIT licensed, Rust 1.81 MSRV, portable procedural macro, and version already present through GPUI. The latest release listed in its changelog is dated 2025-12-22. Its `debug` feature enables `unicode-xid` 0.2.6 through `derive_more-impl`, adding a new package to the lockfile. The local RustSec audit found no advisory for the existing `derive_more` package. | Reject as a direct dependency for this fixed formatting policy: enabling it adds an unnecessary package for a capability already provided by `std::fmt`. |
| `std::fmt::Debug` | The standard library supports custom structured formatting, and the code already uses it for message bodies and calendar records. It adds no package or platform-specific behavior. | Use small, explicit implementations for sensitive domain types and the identifier macro. Redaction stays at each type boundary, while nested outer values inherit it. |
| `redactable` 0.14.0 and `sensitive-fmt` 0.3.0 | [`redactable`](https://docs.rs/crate/redactable/0.14.0) requires Rust 1.97, above the workspace's Rust 1.95 MSRV. [`sensitive-fmt`](https://docs.rs/crate/sensitive-fmt/0.3.0) is a newer project whose maintenance and security history are not established. | Reject: neither improves fit enough to justify its MSRV incompatibility or added project risk. |

### Native message move — 2026-11-01

No new crate is needed for issue #33. Microsoft Graph v1.0 provides a message move action with a `destinationId`; the existing private Reqwest transport, Serde response parsing, GPUI Kit `DropdownMenu` and `PopupMenuItem`, and SQLCipher cache transaction cover the operation. The application can coordinate account and folder reservations without a general-purpose queue library. The move uses delegated `Mail.ReadWrite`, which the provider already requests for message updates. A generic Graph SDK would not replace the app's strict unknown-outcome policy or transactional cache reconciliation, so the implementation keeps the existing boundary.

### GPUI Kit accessibility test support — 2026-11-07

GPUI Kit 0.9.0 provides `test-support`, `TestSupportExt`, and `TestQueryExt` for headless UI and accessibility-tree checks. Enable the feature only on the desktop crate's development dependency and use Kit's `#[gpui_kit::test]` macro. This avoids a separate accessibility test framework or app-owned query layer. The feature remains test-only. The current lockfile has `taffy` 0.15.0 and `unicode-properties` 0.1.3, as it did before the 0.9.0 upgrade. GPUI Fast is inactive on Linux; `cargo tree --target x86_64-unknown-linux-gnu --edges all -i gpui-fast` finds no active path, and the desktop uses GPUI Pre. Taffy 0.15.0 is active through GPUI Pre, and Rustybuzz consumes Unicode properties 0.1.3 on the text-rendering path. The pinned [Kit testing guide](../../.agents/skills/gpui-kit/references/gpui/test.md) documents the feature and query API. The headless checks cover the calendar agenda's listbox name, option labels, focus, and selection; Kit 0.9.0 still does not expose option position or set size, so those properties remain unverified.

### GPUI Kit DatePicker accessible names — 2026-11-07

GPUI Kit 0.9.0's styled `DatePicker` builds a `gpui-base::DatePicker` ComboBox. Base exposes `aria_label`, but the styled component has no accessible-name builder. Applications cannot name the focusable control through the component API. Keep the styled picker and patch the existing package through `[patch.crates-io]`. Add an optional `accessibility_label` builder and pass its value to the Base element. The patch adds no package or dependency edge. The Apache-2.0 source copy occupies about 4.5 MiB. `scripts/check-gpui-component-vendor.sh` verifies it against the published package plus the local patch. Remove the patch when a release provides the same builder, and keep the headless accessibility-tree regression test. Issue #22 tracks real screen-reader verification. See [ADR 0050](../decisions/0050-name-gpui-date-pickers-through-component-patch.md).

### Plain-text message recipient validation — 2026-11-01

| Candidate | Evidence | Decision |
| --- | --- | --- |
| `addr-spec` 0.10.4 | Apache-2.0 Rust parser for one RFC 5322 `addr-spec`, with UTF-8 and NFC normalization support. It declares Rust 1.81 and its upstream repository had recent activity on 2026-10-25. The default feature adds `unicode-normalization`; `rustc_version` is a small build dependency. It parses the whole value and does not accept display names. Its parser uses internal unsafe storage code. | Use for the composer’s single bare recipient, trimming only outer UI whitespace and storing the library’s normalized serialization. This avoids a general header parser and keeps the parser boundary in the domain crate. The app calls only the safe `FromStr` API and adds no unsafe code. `email-address-parser` 2.0.2 was considered, but its `quick-xml` 0.39.4 build dependency is affected by RUSTSEC-2026-0194 and RUSTSEC-2026-0195; its caret requirement excludes patched 0.41.0. `email_address` 0.2.9 has a smaller API but its last stable release was 2024-07-31. |
| `emailaddr` 1.0.0 | MIT OR Apache-2.0, Rust 1.86, strict `EmailAddr`, SMTP address-length rules, and IDNA normalization. Its default `std` feature enables `idna` and `thiserror`; its source also contains internal unsafe casts. The current release history does not yet show an established maintenance cadence. | Considered as a recently released option, but its SMTP and IDNA policy is broader than the app’s RFC 5322 `addr-spec` need. Its dependencies and unsafe storage implementation do not improve this single bare-address boundary over `addr-spec` 0.10.4. |
| `email-address-parser` 2.0.2 | MIT-licensed RFC 5322/6532 parser with strict address-only parsing. Its stable release has no declared MSRV; 3.0 remains a release candidate. | Do not use while its `quick-xml` 0.39.4 build dependency is affected by RUSTSEC-2026-0194 and RUSTSEC-2026-0195. Its caret requirement excludes patched 0.41.0. |
| `email_address` 0.2.9 | Small MIT-licensed API, UTF-8 support, and no required runtime dependencies when Serde is disabled. | Do not select: its last stable release was 2024-07-31, and it does not normalize equivalent address forms. |
| `lettre` and `mail-builder` | Maintained mail composition libraries with SMTP/MIME and RFC message-building APIs. | Not suitable for the Graph adapter, which posts a JSON message object and does not use SMTP or MIME construction. |

The parser validates syntax only; it cannot establish that an address exists or can receive mail. The request path still treats the recipient and all message fields as untrusted input. `addr-spec` contains unsafe code internally; the app uses its safe parser API and does not introduce unsafe Rust. Run RustSec against the exact lockfile after adding the selected crate. See [ADR 0017](../decisions/0017-safe-plain-text-mail-sending.md) for the send boundary and its failure policy.

| Need | Candidate and evidence | Status and next check |
| --- | --- | --- |
| App-generated entity identifiers | `uuid` 1.29.0 is actively developed under Apache-2.0 or MIT and has a Rust 1.89 MSRV. UUID v4 generation is feature-gated and uses operating-system randomness; v7 adds timestamp ordering. Both fit this workspace's MSRV. | Use `uuid` with defaults disabled and only `std` to parse the Entra client ID. Do not generate app IDs; account identity remains the opaque Graph `id` under ADR 0009. |
| Microsoft Graph client | Microsoft does not list Rust among its Graph SDK languages or Kiota generators. `graph-rs-sdk` 3.0.1 has broad Graph coverage, paging, and delta support. Its latest stable release is dated 2025-04-20. It uses `reqwest` 0.12 and Tokio. Its optional `interactive-auth` feature adds Wry and Tao. `io-msgraph` 0.6.0 has an I/O-free Rust core and broad mail/calendar operations, but its error type drops `Retry-After`, its source logs request URLs at `trace` level, and its published README conflicts with its changelog. `reqwest` 0.13.5 is MIT or Apache-2.0, supports Rust 1.85 or later, and exposes status and headers. Its async client requires Tokio, while GPUI Kit uses `smol` in the active Linux dependency graph. | ADR 0006 accepts `reqwest` and `serde` for the Linux Graph adapter. The provider owns a private Tokio worker, HTTP and Graph types, response limits, and token handling. It serves profile, folder, message, and calendar-view delta reads plus explicit message PATCH actions. Production adapter tests use synthetic HTTP fixtures for folder pagination and traversal, sparse and removed message-delta records, redirect rejection, retry behavior, response limits, cancellation, shutdown, proxy routing, and error redaction. The duplicate test-only Graph client was removed; no live Graph behavior is claimed. Tenant-dependent behavior remains unverified under issues #3, #6, #8, #24, and #32. |
| Microsoft Graph entity-tag parsing | [`headers` 0.4.2](https://docs.rs/headers/0.4.2/headers/) is maintained by Hyperium, MIT-licensed, and declares Rust 1.57. Its typed `ETag` parser supports strong and weak entity tags and converts one parsed tag to `IfMatch`. It adds three locked packages: `headers`, `headers-core`, and `sha1`; its other runtime dependencies already exist in the lockfile. The `sha1` dependency supports other typed headers and is not used by this ETag path. [`etag` 4.0.0](https://docs.rs/etag/4.0.0/etag/) imposes a 62-byte tag limit, which Graph does not document as an upper bound. | Use `headers::ETag` at the Graph response boundary and serialize the typed value as a single `If-Match` condition. This rejects wildcard, whitespace-wrapped wildcard, unquoted, and malformed values before sending a request. The synthetic transport test also confirms a valid weak ETag is sent unchanged. `HeaderValue` alone only validates header bytes and is not an entity-tag parser. The 2026-11-05 Linux gate passed the Rust 1.95 workspace check, Clippy, rustdoc, and workspace tests. `cargo audit --deny yanked` scanned 921 packages with no vulnerable or yanked versions and reported the same eight previously allowed unmaintained dependencies. |
| Microsoft 365 desktop authentication | Microsoft's published MSAL list does not include Rust. The Microsoft `azure_identity` crate documents developer-tool, Azure-hosted, and service-principal credentials, not an interactive desktop public-client flow. `oauth2` 5.0.0 (MIT OR Apache-2.0, Rust 1.65) implements authorization-code with PKCE and refresh. `openidconnect` 4.0.1 (MIT, Rust 1.65) adds discovery and ID-token validation, but does not validate `azp`. It also depends on `rsa`, which has an unpatched RustSec Marvin timing advisory. Neither protocol crate starts the system browser or owns the loopback callback. Their built-in `reqwest` adapter targets 0.12; `oauth2-reqwest` 0.1.0-alpha.3 adapts `oauth2` to `reqwest` 0.13 but is pre-release. The OAuth 2 crate warns against following redirects in its token HTTP client because that can enable SSRF. Huskarl 0.11.10 implements authorization-code with PKCE, state, OIDC nonce, and issuer/audience checks. Its `huskarl-reqwest` 0.9.3 adapter uses `reqwest` 0.13. | ADR 0003 selects Huskarl 0.11.10 with default features disabled for authorization code with PKCE, state, callback, and refresh. Production requests no OIDC scopes and consumes no ID tokens. Use `huskarl-reqwest` 0.9.3 with redirects disabled, `reqwest` 0.13 for Graph, and `keyring` 4.2's v1 Linux Secret Service backend. The browser launcher uses `webbrowser` 1.2.6 or later with `hardened`. Synthetic provider tests cover the account flow and profile mapping. Issue #5 is closed after the local Linux gate; tenant evidence remains open in issue #3. |
| System-browser launch | `webbrowser` 1.2.6 (MIT OR Apache-2.0, Rust 1.85; released 2026-10-22) supports the default browser on Windows, macOS, and Linux. Its `hardened` feature rejects non-HTTP(S) URLs. RustSec RUSTSEC-2026-0257 affects versions through 1.2.1; 1.2.2 and later fix the Unix `BROWSER` argument-injection issue. | Selected as a Linux runtime dependency. Synthetic tests verify HTTPS URL handling and reject `file:` URLs without launching a desktop browser. Default-browser and Entra behavior remain open in issue #3. Defer Windows and macOS validation. |
| Account-operation cancellation | `tokio-util` 0.7.20 (MIT, Rust 1.85) is maintained in the Tokio repository; its 2026-10-09 release fixes cancellation-token descendant notification ordering. `CancellationToken` supplies shared, asynchronously awaitable cancellation. The crate has no native build requirements and is portable Rust; only Linux is currently in scope. Its default feature set is empty, and the application uses the workspace's existing `rt` feature selection. The application adds a direct edge to a crate already used by the Microsoft provider, so this does not add another lockfile package. The current lockfile RustSec result is recorded in the candidate review below after the Linux gate. | Use `CancellationToken` for notification and an application-owned atomic phase for the product-specific cancellation-versus-credential-commit ordering. The phase is not delegated to the dependency. Compared with a custom `Future` and `Waker`, this reuses maintained cancellation signaling already present in the workspace. Compared with `futures_util::future::select`, it provides a cloneable shared cancellation handle that crosses the application/provider boundary. No adequate `std` cancellation primitive exists. Synthetic tests cover both winning orders and preserved credentials. Recheck the pinned crate on each dependency update. See [Tokio `tokio-util` 0.7.20](https://github.com/tokio-rs/tokio/tree/tokio-util-0.7.20), its [manifest](https://github.com/tokio-rs/tokio/blob/tokio-util-0.7.20/tokio-util/Cargo.toml), and [cancellation-token API](https://docs.rs/tokio-util/0.7.20/tokio_util/sync/struct.CancellationToken.html). |
| Loopback callback HTTP server | `axum` 0.8.9 (MIT, Rust 1.85; released 2026-04-14) provides maintained HTTP routing and a `with_graceful_shutdown` server API. Its default features include Tokio, Hyper, forms, JSON, and tracing. A callback listener can disable defaults and enable only the required server and query features. `tiny_http` 0.12.0 has not released since 2022-10-06. | Huskarl's authorization-code loopback feature accepts a caller-owned Tokio listener, parses the callback, and releases the listener when the future is cancelled. The desktop app requires a registered nonzero port on `127.0.0.1` under ADR 0031. It does not need a separate callback server crate. |
| Huskarl OAuth desktop flow | `huskarl` 0.11.10 is MIT OR Apache-2.0 and requires Rust 1.92. Its loopback feature accepts a caller-owned Tokio listener and awaits the callback inline. It does not launch the system browser; `webbrowser` remains a separate candidate. The app must own and drop the listener when it cancels sign-in. The tagged CI workflow runs on Ubuntu only. The default verifier feature adds `huskarl-crypto-native` 0.11.3, which pins `rsa` 0.10.0-rc.19 and includes private RSA signing code. RustSec lists no patched RSA release and names rc.18 as affected, so verify rc.19 before adoption. Huskarl exposes `azp` but has a test that accepts a mismatching value. Disabling its default features allows a custom verifier platform. An early test-only experiment added direct `jsonwebtoken` 11.1.0 and AWS-LC edges for custom RS256 verification; the standalone proof and those direct dependency edges were removed because production sets `.oidc(false)` and neither requests nor consumes ID tokens. | The Linux provider uses Huskarl for authorization code with PKCE, state, token exchange, callback handling, and refresh. Default OIDC verifier features stay disabled because the product flow does not request OIDC scopes or consume ID tokens. Synthetic provider tests exercise the production flow, including forged-state and redirect handling, without contacting Entra. Keep issue #3 open for tenant evidence. |
| Other turnkey OAuth loopback helpers | `loopauth` 0.5.1 (MIT OR Apache-2.0) provides PKCE, a loopback callback, token exchange, refresh, and JWKS validation. Its manifest does not declare an MSRV. Its repository last changed on 2026-08-28, and its visible CI runs on Ubuntu only. The API targets CLI token acquisition and leaves app identity and session handling to the caller. It has no public cancellation handle; `run_authorization_flow` detaches the callback server task. The manifest pulls in Axum, `reqwest` 0.13, `webbrowser`, and the full Tokio feature set, as well as Rustls and TLS-certificate support. `oauthkit` 0.2.4 is a CLI SDK with authorization-code/PKCE, device-code, API-key, and credential-storage flows. Its current docs do not provide a verified Windows build, a declared MSRV, or a clear platform CI result. `oauth-toolkit` 0.4.2 (MIT OR Apache-2.0, Rust 1.85; published 2026-10-29) binds `127.0.0.1`, creates PKCE and single-use state, and captures one redirect. Its source implements an HTTP request parser and polls for the callback on a blocking thread. Its manifest adds `reqwest` 0.12, Tokio, `jsonwebtoken`, and broad provider features. The latest [RustSec run](https://github.com/WyattAu/oauth-toolkit/actions/runs/41157846278) reports six advisories, including one unsoundness advisory and one other advisory. | No helper is selected. `loopauth` has a useful integrated flow, but its detached server cannot be cancelled by its caller and Windows behavior is unverified. `oauthkit` has the same Windows evidence gap and no checked MSRV. `oauth-toolkit` is active but reimplements HTTP parsing, duplicates the planned `reqwest` line, and has a failing audit. Prefer maintained protocol, browser, and callback libraries. |
| Operating-system credential storage | [`keyring` 4.2.0](https://docs.rs/crate/keyring/4.2.0) is MIT OR Apache-2.0, declares Rust 1.88, and its `v1` API sets, gets, and deletes binary secrets. On Linux it selects the zbus Secret Service store. Its v1 API also has native Windows Credential Manager and macOS Keychain adapters for later milestones. The Linux feature graph adds `zbus-secret-service-keyring-store` and `secret-service` through `zbus`; zbus is already present through GPUI's Linux stack. Keep the `cli` feature disabled. [`keyring-core` 1.0.1](https://docs.rs/crate/keyring-core/1.0.1) is MIT OR Apache-2.0 and declares Rust 1.85. [`oo7` 0.6.0](https://docs.rs/oo7/0.6.0/oo7/) is an MIT-licensed alternative with Secret Service and encrypted file/portal backends. It is async-only and documents Tokio, async-std, or GLib runtimes; its sandbox-focused file backend is not needed by the current unsandboxed GPUI app. | The Linux Microsoft provider uses `keyring` 4.2.0 with `v1` and without `cli` to store its refresh token in Secret Service. Its ignored production-path test covers save, replace, load, restore, rotation, sign-out, and delete using synthetic credentials. Linux persistence tests now cover the SQLCipher key lifecycle through `SqliteMailCache`; both test paths use synthetic credentials and no real mailbox data. |
| Local encrypted mail cache | `rusqlite` 0.41.1 is the latest release as of 2026-10-31. It is MIT-licensed, declares Rust 1.89, and exposes bundled SQLCipher features. `rusqlite` 0.40.2 is the latest 0.40 patch and is the compatible line for the reviewed migration and async crates, which require `rusqlite ^0.40.0`. Bundled SQLCipher is BSD-licensed and compiles native C code. The vendored OpenSSL feature adds an OpenSSL source build that needs a C compiler, Perl, `Text::Template`, and make; Windows builds also need MSVC and NASM. `SQLx` 0.9.0 does not document a supported SQLCipher SQLite configuration. `redb` 4.4.0 lacks built-in data-at-rest encryption, and Turso labels encryption experimental. | Production Linux persistence uses `async-sqlite` 0.6.0 with defaults disabled and `bundled-sqlcipher-vendored-openssl`, `rusqlite_migration` 2.6.0, and Linux `keyring` 4.2.0 `v1` for Secret Service keys. The async wrapper is MIT, declares Rust 1.95, and provides one bounded background-worker connection; the migration crate is Apache-2.0, declares Rust 1.95, and applies migrations atomically. Synthetic persistence tests cover encrypted records, page/cursor transactions, restart checkpoints, body caching, cache-file removal, and the production cache-key lifecycle. No real mailbox content is used. |
| Calendar occurrence times and Linux display | Graph's v1.0 `calendarView` returns expanded occurrences in UTC by default and retains original source-zone IDs on each event. The event resource requires all-day start and end values to be midnight in the same time zone. The docs do not say how a non-UTC all-day event appears in a UTC-default response. Jiff 0.2.39 (MIT OR Unlicense, MSRV 1.70, pre-1.0; released 2026-11-02) converts timed instants to zoned date-times and reads the host zoneinfo database on Unix. The desktop manifest enables only Jiff's `std`, `tz-system`, and `tzdb-zoneinfo` features. The Linux feature graph adds no bundled time-zone database; `serde` is unified by another dependency. Jiff's changelog records a soundness fix in 0.2.30; 0.2.39 includes it and fixes a panic in `Timestamp::from_nanosecond` for out-of-range inputs. The reviewed RustSec database has no Jiff advisory entry. | ADR 0014 selects Graph-expanded UTC occurrences plus Jiff for Linux timed-event display. The calendar view uses Jiff only at the desktop boundary. Domain and application types do not depend on Jiff. All-day events use date ranges and remain unchanged; the current UTC-midnight mapping is covered by synthetic fixtures only. Preserve source IDs as opaque metadata; defer Windows-to-IANA mapping and local recurrence expansion. Keep issue #6 open for live Graph recurrence and all-day verification. Revisit the Linux-only Jiff choice when Windows or macOS work begins. |
| GPUI Kit calendar picker boundary | Chrono 0.4.45 is MIT OR Apache-2.0, declares Rust 1.62, and had an active signed release on 2026-06-04. The desktop's direct edge disables default features and enables `std`; GPUI Kit 0.9.0 already depends on Chrono and exposes `DatePickerState::date_time` as `NaiveDateTime`, so this adds no resolved package. The picker uses Chrono only as its public UI value type. The Linux time-zone resolver remains Jiff, while the domain and Graph values use `time`. RustSec reports no Chrono advisory in the reviewed lockfile. | Keep the direct Chrono edge in `apps/desktop` because the picker API returns its type and does not re-export it. Convert at the desktop boundary; do not use Chrono for time-zone resolution, domain values, or Graph serialization. GPUI Kit's `DatePicker` already provides the needed minute-precision time editor, so no separate picker crate or custom general-purpose time control is needed. |
| RFC 5322 and MIME parsing | `mail-parser` 1.0.0 handles nested messages and MIME parts. Its current MSRV is Rust 1.98, above this workspace's Rust 1.95 declaration. | Defer unless the app consumes raw MIME. Microsoft Graph message JSON already returns body content. If raw MIME becomes a requirement, compare it with `mailparse` and decide whether the MSRV change is justified. |
| Untrusted HTML sanitization | `ammonia` 4.2.4 (MIT OR Apache-2.0, Rust 1.85) uses `html5ever` 0.40, `cssparser` 0.38, `maplit` 1, and `url` 2. It fits the workspace MSRV of Rust 1.95. RustSec records historical advisories; 4.2.4 includes the fix for RUSTSEC-2026-0373. `htmlsanitizer` 0.2.0 (MIT) is not a full HTML parser, does not state its MSRV in the reviewed metadata, and has a shorter release history. | ADR 0007 accepts Ammonia plus native GPUI `TextView::html` for Linux. The application sanitizes at display time and keeps raw bodies encrypted. `html2text` 0.17.4 provides the bounded oversized-body fallback. Synthetic application and headless `MailView` tests cover sanitizer edge cases, unsafe links, image blocking, selection, copy, and truncation. The redundant standalone HTML proof package was removed. Real-window layout, operating-system clipboard, and browser launch remain open under issue #9. |

For follow-up flag updates, no additional library fits the single Graph-specific PATCH better than the accepted provider boundary. The existing `reqwest` request path and `serde` parser already expose status, headers, and bounded JSON responses; the provider enforces explicit statuses, immutable IDs, no automatic write retries, and matching confirmation. The maintained Graph SDK candidates would add another HTTP/authentication policy layer without removing those checks. The reviewed alternatives and their maintenance, platform, license, MSRV, security, API-fit, and dependency-cost evidence are recorded in the Microsoft Graph client row and the references below. ADR 0019 records the choice; no dependency or lockfile change was needed.

### Production persistence key-lifecycle tests

Linux persistence tests exercise SQLCipher encryption and Secret Service key handling through `SqliteMailCache::open_for_account` and `remove_for_account`. The default child-process case covers unavailable D-Bus for new and existing caches; the ignored case covers key creation, encrypted reopen, wrong and malformed key rejection, missing-key preservation, and account removal. It passed on 2026-11-08 in an isolated GNOME Keyring D-Bus session. The tests use existing workspace dependencies, random synthetic account IDs, and temporary XDG storage; they add no crate or native build requirement.

The original standalone proof was removed after its remaining behaviors were covered by production-path tests. Its backup-specific cases had already been removed because production has no backup workflow. The Microsoft provider separately uses `keyring` for refresh-token storage and retains its own ignored Secret Service lifecycle test.

Local Linux compilation used `OPENSSL_NO_VENDOR=1` because this environment lacks `FindBin.pm`; this uses system OpenSSL. Linux CI installs the vendored OpenSSL build requirements. The required Fedora modules are `perl-FindBin` and `perl-Text-Template`; Ubuntu installs `perl` and `libtext-template-perl`. Windows needs Perl, `Text::Template`, MSVC, and NASM for the vendored OpenSSL build.

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

- Per-account cache lifecycle coordination uses Rust's `std::fs::File::lock` on the existing `mail-cache.lock` file. The standard-library API is available within the Rust 1.95 MSRV and uses the same lock for send staging and cache removal. No additional locking crate is needed for this Linux file boundary.
- The Linux desktop uses `std::fs::File::try_lock` for its startup process lock. Rust stabilized that API in 1.89, below the workspace's 1.95 MSRV. No lock crate is needed because startup acquires the lock synchronously before GPUI starts. The path uses `xdg` 3.0.0, already used by persistence. Its package metadata lists Apache-2.0 OR MIT and Rust 1.60; its only optional feature is `serde`, which the app does not enable. The 3.0.0 release is dated 2025-05-04, so recheck upstream activity when updating the dependency. `xdg::BaseDirectories::place_data_file` creates the XDG data path. Compared with `directories` 6.0.0, `xdg` fits the Linux-only target and already owns the persistence path. `directories` adds cross-platform directory behavior that this milestone does not use. A single-instance wrapper would add a crate around the standard-library lock without adding activation forwarding, which this app does not need.
- `blocking` 1.7.0 runs the synchronous file-lock wait on its blocking-I/O thread pool while persistence awaits the result. The crate is dual MIT/Apache-2.0, declares Rust 1.68, and is maintained by the smol-rs organization; version 1.7.0 was published in 2026 and is already present in the workspace lockfile transitively. Making it a direct persistence dependency adds no new resolved package. `fs4`'s `AsyncFileExt::lock` still invokes a blocking lock syscall, so it does not meet the executor requirement; `async_file_lock` documents process-local locking, so it does not meet the cross-process requirement. See [`blocking`](https://docs.rs/blocking/1.7.0/blocking/), [`fs4`](https://docs.rs/fs4/1.1.0/fs4/trait.AsyncFileExt.html), and [`async_file_lock`](https://docs.rs/async-file-lock/0.1.4/async_file_lock/struct.FileLock.html).
- The Microsoft provider uses Graph, OAuth, browser, HTTP, and Secret Service libraries. Application workflows keep provider-neutral account and mail types. Graph mail access is read-only in M2.
- ADR 0005 selects SQLCipher-backed SQLite. Production Linux persistence uses the reviewed async wrapper and migration crate for encrypted mail records, page/cursor transactions, and account-scoped cache removal. The Windows x64 evidence is historical; current platform work targets Linux.
- Linux is the sole current development and validation target. Windows and macOS implementation work and validation are deferred to later roadmap milestones.
- No dedicated Microsoft 365 test tenant or public app registration is available. Synthetic Graph and OAuth tests do not establish live Entra or mailbox behavior.
- The provider's ignored credential lifecycle test passed in Fedora GNOME Keyring on 2026-10-31. The production persistence cache-key test passed in an isolated GNOME Keyring session on 2026-11-08; default tests cover unavailable D-Bus, and a unit test covers synthetic `NoStorageAccess` classification.
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
- [`xdg` 3.0.0 package metadata](https://docs.rs/crate/xdg/3.0.0)
- [Rust 1.95 `File::try_lock` documentation](https://doc.rust-lang.org/1.95.0/std/fs/struct.File.html#method.try_lock)
- [`directories` 6.0.0 package metadata](https://docs.rs/crate/directories/6.0.0)
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
- [Chrono 0.4.45 package metadata](https://docs.rs/crate/chrono/0.4.45)
- [Chrono 0.4.45 manifest](https://github.com/chronotope/chrono/blob/0.4.45/Cargo.toml)
- [Chrono 0.4.45 release](https://github.com/chronotope/chrono/releases/tag/0.4.45)
- [GPUI Kit v0.9.0 release notes](https://github.com/longbridge/gpui-kit/releases/tag/v0.9.0)
- [GPUI Kit 0.9.0 DatePickerState API](https://docs.rs/gpui-kit/0.9.0/gpui_kit/component/date_picker/struct.DatePickerState.html)
- [`jiff` 0.2.39 manifest](https://github.com/BurntSushi/jiff/blob/0.2.39/crates/jiff/Cargo.toml)
- [`jiff` 0.2.39 workspace manifest and MSRV](https://github.com/BurntSushi/jiff/blob/0.2.39/Cargo.toml)
- [`jiff` 0.2.39 changelog](https://github.com/BurntSushi/jiff/blob/0.2.39/CHANGELOG.md)
- [`jiff` 0.2.39 time-zone and platform documentation](https://docs.rs/jiff/0.2.39/jiff/tz/)
- [`chrono-tz` 0.10.4 package documentation](https://docs.rs/crate/chrono-tz/0.10.4)
- [`chrono-tz` upstream manifest](https://github.com/chronotope/chrono-tz/blob/main/chrono-tz/Cargo.toml)
- [`windows-timezones` 0.5.1 documentation](https://docs.rs/windows-timezones/0.5.1/windows_timezones/)
- [`localzone` 0.3.1 Windows-to-IANA function](https://docs.rs/localzone/0.3.1/localzone/fn.win_zone_to_iana.html)
- [`time-tz` package documentation and release history](https://docs.rs/crate/time-tz/latest)
- [`time-tz` upstream repository and manifest](https://github.com/Yuri6037/time-tz)
- [ICU4X `TimeZone` Windows-ID mapping](https://docs.rs/icu/latest/icu/time/zone/struct.TimeZone.html)
- [ICU4X time-zone API limits](https://docs.rs/icu/latest/icu/time/zone/index.html)
