# Project status

Last updated: 2026-11-09

## Current milestone

M7 Exchange Server EWS support is in progress under [issue #257](https://github.com/1fach/einfach-mail/issues/257). The first implementation targets Linux read-only mail and keeps Exchange Online on Microsoft Graph. ADR 0053 records the accepted provider boundary, authentication, TLS, and account-capability decisions. Linux Autodiscover merged into `main` in [PR #261](https://github.com/1fach/einfach-mail/pull/261) and remains tracked in [issue #258](https://github.com/1fach/einfach-mail/issues/258) for live verification.

Issue [#266](https://github.com/1fach/einfach-mail/issues/266) and [ADR 0057](../decisions/0057-read-only-ews-calendar-snapshots.md) track M8 EWS calendar work. The provider builds a bounded `FindItem` calendar view and maps complete synthetic responses. The application workflow now selects Graph delta or EWS snapshot synchronization, and the SQLCipher adapter atomically replaces one exact view with separate snapshot completion state. The current branch exposes EWS calendar reads through the shared native agenda, displays the read-only capability, and hides and guards event writes. The Linux local gate passed on 2026-11-09, including the focused account and agenda checks. No Exchange calendar server or sanitized response recording is available for live verification.

PR #47 merged read-only mail into `main` at `409d299ffbe11dfe41c8f5a428b8e49b9ac97edd` on 2026-11-06. PR #50 merged the Linux calendar agenda and its reviewed stack into `main` at `8c10dca99fd8cc0a38f76fb11aed5a174b0970f8` on 2026-11-06. All four required PR #50 checks passed on head `0ecfc019ff661282bc6bf5bfe5ce522bbb77c4e3`.

PR #51 merged issue #49 calendar cursor recovery into `main` at `a31c775fc95e23d0b705efb6b8f7560b60c45e6f` on 2026-11-06. All four required checks passed on head `3b99b11a708a80bb02ef82553e2144165bd88ab7`. ADR 0036 records the reset flow, ADR 0043 records the per-account session boundary, and ADR 0044 records the event-action gate during replacement scans. Keep issue #49 open until a Microsoft 365 tenant confirms the cursor-reset response shape.

[PR #79](https://github.com/1fach/einfach-mail/pull/79) and [PR #80](https://github.com/1fach/einfach-mail/pull/80) added headless accessibility snapshots for the agenda and message list. [PR #81](https://github.com/1fach/einfach-mail/pull/81) merged into `main` at `54df53621d0bc4ac337cde88a68def0b2c5a9bd4` on 2026-11-07. It adds synthetic Graph coverage for recurring occurrences across a daylight-saving transition and completes issue #6's synthetic fixture criterion. Issue #6 remains open for live recurrence, time-zone, and all-day verification.

[PR #246](https://github.com/1fach/einfach-mail/pull/246) implements issue #245's durable per-view recovery for calendar event writes. Local Linux checks pass, including SQLCipher reopen and partial-view recovery tests. Live Graph behavior remains unverified under issues #40 and #42.

[PR #260](https://github.com/1fach/einfach-mail/pull/260) implements issue #257's Linux read-only EWS path. The Linux-local gate passed on 2026-11-09, including synthetic HTTPS NTLM and SOAP request-response tests. No UI or GitHub Actions run was used. Keep issue #257 open until a real Exchange Server or sanitized live-response recording verifies the connection and response behavior.

Issue #258 tracks Linux EWS Autodiscover. Its implementation and synthetic verification merged into `main` at `a8226c83770e70d1bbd94767b1a5cb6f15fb7c23` in PR #261, with the accepted request and credential boundary in ADR 0054. The full Linux local gate passed on 2026-11-09, including Rust 1.95, formatting, Clippy, rustdoc, dependency policy, audit, and workspace tests. The SQLCipher advisory exception has since been removed; PR #265 records the patched runtime and legacy-cache verification for issue #262. No Exchange Server is available, so keep #258 open for live server verification.

PR #265 contains the locally verified Linux implementation for issue #262. Runtime tests report SQLCipher 4.20.0 on SQLite 3.54.1. The production adapter migrates a synthetic SQLCipher 4.14.0 cache and preserves its message body; the production Secret Service key lifecycle also passes on the current dependency stack in a private headless D-Bus session. The full Linux local gate passes with RUSTSEC-2026-0459 removed. Issue #262 documents a one-time Linux-only exception for exact commits from upstream draft PRs #1932 and #370. Follow-up issue [#264](https://github.com/1fach/einfach-mail/issues/264) tracks replacing the pins after compatible official releases. No project GitHub Actions or UI launch was used.

## Active risks

- No dedicated Microsoft 365 test tenant or public app registration is available. Keep live sign-in, initial mail sync, read-state, flag, send, reply, move, and calendar verification open under their existing issues.
- No Exchange Server test host or sanitized EWS response recording is available. Synthetic EWS tests can verify request construction and mapping, but cannot prove the NTLM channel-binding handshake or server-specific response behavior. Keep issue #257 open for that live verification.
- No Exchange Server test host is available to verify Autodiscover's published candidate sequence, SRV behavior, redirects, and server response variants end to end. Keep issue #258 open for that live verification.
- No Exchange Server test host or sanitized calendar response recording is available to verify EWS recurrence expansion, event time-zone mapping, all-day dates, or calendar result completeness. Keep issue #266's live-compatibility claim unverified until such evidence exists.
- `windows-timezones` 0.5.1 declares no MSRV. The EWS provider compiled with the workspace Rust 1.95, and the Unicode-DFS-2016 terms are retained in [`LICENSES/Unicode-DFS-2016.txt`](../../LICENSES/Unicode-DFS-2016.txt). Custom or unknown Windows time-zone IDs make an EWS snapshot fail.
- SSPI 0.23.1 pins `rsa 0.10.0-rc.18`, flagged by RUSTSEC-2023-0071. The EWS provider uses only SSPI's NTLM path and has no RSA private-key operation; `.cargo/audit.toml` narrowly ignores this advisory. Issue #259 tracks removing the exception after SSPI adopts a fixed stable release.
- Issue #6 has synthetic coverage for expanded occurrences and UTC-midnight all-day inputs. Graph's docs do not say how a non-UTC all-day event appears in a UTC-default `calendarView` response. Live recurrence, time-zone, and all-day behavior remain unverified until a Microsoft 365 tenant is available.
- Manual Linux review remains open under issues #9 and #22. It covers real-window layout, focus, scrolling, screen-reader behavior, clipboard, and browser launch. No GUI has been launched during this work.
- The event composer keeps visible `Starts` and `Ends` labels, but its date pickers lack distinct accessible names after removal of the local component copy. Issue [#176](https://github.com/1fach/einfach-mail/issues/176) tracks an upstream or smaller maintained fix; issue #22 tracks real-window screen-reader review.
- Windows and macOS support remain deferred.
- The production-path cache-key lifecycle test passed on 2026-11-09 against a temporary GNOME Keyring service on the SQLCipher 4.20.0 dependency stack. This does not establish behavior for every Secret Service implementation.
- The 2026-11-09 offline RustSec audit used cached database revision `ba4d1245f77987d28e7a7f119ed21ad844c11f57`, loaded 1,429 advisories, and scanned 1,031 locked dependencies. `cargo audit --deny yanked` passed with nine unmaintained-crate notices and the narrow `rsa` exception tracked in issue #259. The sandbox could not refresh the database because its lock file under the read-only Cargo home was unavailable; the successful result used the repository's supported offline mode.
- The SQLCipher remediation uses exact commits from draft PRs #1932 and #370 under the one-time exception recorded in issue #262. The upstream rusqlite Linux CI is waiting on unpublished sqlite-wasm-rs 0.6.3; this project keeps that optional WebAssembly path disabled. The pins live in contributor forks and need replacing with official releases under issue [#264](https://github.com/1fach/einfach-mail/issues/264). Runtime versions, legacy-cache migration, OS-key lifecycle, and the Linux local gate pass. RUSTSEC-2026-0459 is no longer ignored. See the [dependency review](dependency-review.md) and [ADR 0056](../decisions/0056-pin-patched-sqlcipher-integration.md).
- GPUI Kit v0.9.0 remains the latest release. Upstream `main` is 70 commits ahead at [commit `dbb0b9a`](https://github.com/longbridge/gpui-kit/commit/dbb0b9a4895c4b45753ed8647cb0dbb58178bd96), but its manifest still pins `gpui-pre` 0.3.11 and its lockfile still contains all eight previously known advisory crate names. Recheck when GPUI Kit or its Linux renderer, HTTP, or zbus dependencies publish compatible updates. See the [dependency review](dependency-review.md) for paths and replacement checks.

## Next actions

1. Keep issues #257 and #258 open until a real Exchange Server or sanitized live-response recording verifies the EWS connection and Autodiscover flows.
2. Replace the SQLCipher and migration Git pins with released crates under issue [#264](https://github.com/1fach/einfach-mail/issues/264) when compatible official releases are available.
3. Remove the narrow RSA advisory exception when a maintained SSPI release resolves to a fixed stable RSA version, as tracked in issue #259.
4. Resume the tenant-backed checks in issues #3, #6, #8, #24, #26, #28, #30, #32, #34, #36, #38, #40, #42, and #49 when the required Microsoft 365 test tenant and public app registration are available.
5. Continue the Linux real-window accessibility review tracked in issues #9 and #22.
6. Revisit the DatePicker accessible names in issue #176 when GPUI Kit exposes a suitable API or a smaller maintained solution is available.
7. Recheck the transitive RustSec warnings when GPUI Kit or its Linux renderer, HTTP, or zbus dependencies publish compatible updates.
8. Keep issue [#266](https://github.com/1fach/einfach-mail/issues/266) open until a real Exchange server or sanitized response verifies the EWS calendar request, recurrence expansion, time-zone mapping, and result completeness.

## Update rule

Update this file when the milestone, an active risk, or a next action changes. Keep implementation plans and acceptance criteria in GitHub issues.
