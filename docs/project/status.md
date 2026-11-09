# Project status

Last updated: 2026-11-09

## Current milestone

M7 Exchange Server EWS support is in progress under [issue #257](https://github.com/1fach/einfach-mail/issues/257). The first implementation targets Linux read-only mail and keeps Exchange Online on Microsoft Graph. ADR 0053 records the accepted provider boundary, authentication, TLS, and account-capability decisions. Autodiscover is deferred to [issue #258](https://github.com/1fach/einfach-mail/issues/258).

PR #47 merged read-only mail into `main` at `409d299ffbe11dfe41c8f5a428b8e49b9ac97edd` on 2026-11-06. PR #50 merged the Linux calendar agenda and its reviewed stack into `main` at `8c10dca99fd8cc0a38f76fb11aed5a174b0970f8` on 2026-11-06. All four required PR #50 checks passed on head `0ecfc019ff661282bc6bf5bfe5ce522bbb77c4e3`.

PR #51 merged issue #49 calendar cursor recovery into `main` at `a31c775fc95e23d0b705efb6b8f7560b60c45e6f` on 2026-11-06. All four required checks passed on head `3b99b11a708a80bb02ef82553e2144165bd88ab7`. ADR 0036 records the reset flow, ADR 0043 records the per-account session boundary, and ADR 0044 records the event-action gate during replacement scans. Keep issue #49 open until a Microsoft 365 tenant confirms the cursor-reset response shape.

[PR #79](https://github.com/1fach/einfach-mail/pull/79) and [PR #80](https://github.com/1fach/einfach-mail/pull/80) added headless accessibility snapshots for the agenda and message list. [PR #81](https://github.com/1fach/einfach-mail/pull/81) merged into `main` at `54df53621d0bc4ac337cde88a68def0b2c5a9bd4` on 2026-11-07. It adds synthetic Graph coverage for recurring occurrences across a daylight-saving transition and completes issue #6's synthetic fixture criterion. Issue #6 remains open for live recurrence, time-zone, and all-day verification.

[PR #246](https://github.com/1fach/einfach-mail/pull/246) implements issue #245's durable per-view recovery for calendar event writes. Local Linux checks pass, including SQLCipher reopen and partial-view recovery tests. Live Graph behavior remains unverified under issues #40 and #42.

[PR #260](https://github.com/1fach/einfach-mail/pull/260) implements issue #257's Linux read-only EWS path. The Linux-local gate passed on 2026-11-09, including synthetic HTTPS NTLM and SOAP request-response tests. No UI or GitHub Actions run was used. Keep issue #257 open until a real Exchange Server or sanitized live-response recording verifies the connection and response behavior.

Issue #258 tracks Linux EWS Autodiscover. Its implementation and synthetic verification are complete on the review branch, with the accepted request and credential boundary in ADR 0054. Rust 1.95 compile, formatting, Clippy, rustdoc, dependency policy, the Autodiscover headless checks, and the workspace test suite passed; ignored Secret Service session tests remain unrun. The latest full local gate stopped at `cargo audit` after the advisory database update described below; rerun it after issue #262 is resolved. No Exchange Server is available, so keep #258 open for live server verification after review.

## Active risks

- No dedicated Microsoft 365 test tenant or public app registration is available. Keep live sign-in, initial mail sync, read-state, flag, send, reply, move, and calendar verification open under their existing issues.
- No Exchange Server test host or sanitized EWS response recording is available. Synthetic EWS tests can verify request construction and mapping, but cannot prove the NTLM channel-binding handshake or server-specific response behavior. Keep issue #257 open for that live verification.
- No Exchange Server test host is available to verify Autodiscover's published candidate sequence, SRV behavior, redirects, and server response variants end to end. Keep issue #258 open for that live verification.
- SSPI 0.23.1 pins `rsa 0.10.0-rc.18`, flagged by RUSTSEC-2023-0071. The EWS provider uses only SSPI's NTLM path and has no RSA private-key operation; `.cargo/audit.toml` narrowly ignores this advisory. Issue #259 tracks removing the exception after SSPI adopts a fixed stable release.
- Issue #6 has synthetic coverage for expanded occurrences and UTC-midnight all-day inputs. Graph's docs do not say how a non-UTC all-day event appears in a UTC-default `calendarView` response. Live recurrence, time-zone, and all-day behavior remain unverified until a Microsoft 365 tenant is available.
- Manual Linux review remains open under issues #9 and #22. It covers real-window layout, focus, scrolling, screen-reader behavior, clipboard, and browser launch. No GUI has been launched during this work.
- Windows and macOS support remain deferred.
- The production-path cache-key lifecycle test passed on 2026-11-08 against a temporary GNOME Keyring service. This does not establish behavior for every Secret Service implementation.
- Before the RustSec advisory database refresh, the 2026-11-09 local audit loaded 1,422 advisories and scanned 1,021 locked dependencies. It reported nine informational unmaintained-crate advisories and the narrowly ignored `rsa` advisory tracked in issue #259. The refreshed database's additional finding is listed below.
- The updated 2026-11-09 RustSec audit loaded 1,426 advisories and found pre-existing `libsqlite3-sys 0.38.2` / `RUSTSEC-2026-0459` in `main` as well as this branch. The finding affects SQLite before 3.54.0 when opening an attacker-crafted database and WAL pair. The current bundled SQLCipher source also needs verification: the advisory says `libsqlite3-sys 0.39.0` and `0.39.1` still bundle SQLCipher with SQLite 3.53.4, while upstream SQLCipher 4.20.0 updates to SQLite 3.54.1. Do not treat a `libsqlite3-sys` version bump as remediation unless the actual bundled SQLCipher baseline is verified. Issue [#262](https://github.com/1fach/einfach-mail/issues/262) tracks the fix and the required audit remains failing until it is resolved. The Autodiscover PR does not change this dependency.
- GPUI Kit v0.9.0 remains the latest release. Upstream `main` is 70 commits ahead at [commit `dbb0b9a`](https://github.com/longbridge/gpui-kit/commit/dbb0b9a4895c4b45753ed8647cb0dbb58178bd96), but its manifest still pins `gpui-pre` 0.3.11 and its lockfile still contains all eight previously known advisory crate names. Recheck when GPUI Kit or its Linux renderer, HTTP, or zbus dependencies publish compatible updates. See the [dependency review](dependency-review.md) for paths and replacement checks.

## Next actions

1. Keep issues #257 and #258 open until a real Exchange Server or sanitized live-response recording verifies the EWS connection and Autodiscover flows.
2. Resolve issue #262 and rerun the `cargo audit --deny yanked` local gate.
3. Remove the narrow RSA advisory exception when a maintained SSPI release resolves to a fixed stable RSA version, as tracked in issue #259.
4. Resume the tenant-backed checks in issues #3, #6, #8, #24, #26, #28, #30, #32, #34, #36, #38, #40, #42, and #49 when the required Microsoft 365 test tenant and public app registration are available.
5. Continue the Linux real-window accessibility review tracked in issues #9 and #22.
6. Recheck the transitive RustSec warnings when GPUI Kit or its Linux renderer, HTTP, or zbus dependencies publish compatible updates.

## Update rule

Update this file when the milestone, an active risk, or a next action changes. Keep implementation plans and acceptance criteria in GitHub issues.
