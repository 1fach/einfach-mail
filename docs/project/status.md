# Project status

Last updated: 2026-11-04

## Current milestone

M2 read-only mail remains in draft review in PR #47. Its current head, `c09ccdd92fc4597b4130e86a957b28f7fdac624f`, passes all four required GitHub checks. The reviewed follow-up adds shared account and folder reservations, persisted checkpoint revisions, and synthetic concurrency regressions for stale responses. The full Linux project gate passed locally on 2026-11-04, including workspace tests, Clippy, rustdoc, RustSec audit, and dependency-policy checks. Formatting, all 18 desktop tests, and workspace rustdoc passed again after the final copy and Rustdoc edits. The lockfile uses upstream-fixed `async-executor` 1.14.1. No live Entra or Graph request has been made.

M3 calendar integration has started on a clean branch based on the passing PR #47 head. The earlier calendar implementation does not apply cleanly because it predates the current mail-session and persistence boundaries. ADRs 0014 and 0038 record the calendar scope and the shared per-account session decision before implementation.

PR #46 merged into `main` on 2026-11-02. It contains the M1 account connection and cancellation boundary. Keep issue #3 open until live Entra checks pass.

## Verified state

- The Linux GPUI Kit foundation and encrypted-cache/Secret Service proof are merged. Linux is the current development and validation target; Windows and macOS support are deferred under [ADR 0010](../decisions/0010-linux-first-development.md).
- The account view uses an application workflow and provider-neutral profile. The Microsoft provider uses authorization code with PKCE, Graph `/me`, and Linux Secret Service for refresh credentials. Cancellation is ordered against credential commits so the view reports the operation's actual outcome.
- The M2 candidate contains read-only folder and message synchronization, a fixed 90-day cutoff, encrypted transactions that commit each page and cursor together, fetch-on-open bodies, offline cached browsing, keyboard navigation, and native rendering of sanitized message content. Expired message delta cursors start a durable replacement scan while cached messages remain visible. The candidate contains no calendar workflows or mail-write actions.
- Sessions created by one mail workflow factory share account and folder reservations. A sync reserves the account before folder enumeration and holds it through folder replacement and delta sync. Each persisted page or reset advances a checkpoint revision, and stale responses cannot advance or finalize a newer synchronization state.
- Automated fixtures use synthetic data. The candidate includes Graph transport, persistence, sanitizer, and headless GPUI proof packages. The mail fixes pass the Rust 1.95 workspace/all-target check, full workspace tests, affected-crate Clippy, rustdoc, and formatting. Dependency-boundary and banned-framework checks pass. After CI identified yanked `async-executor` 1.14.0, the lockfile was updated to upstream-fixed 1.14.1; a fresh RustSec audit loaded 1,403 advisories and found no vulnerable or yanked locked crates, while reporting the eight upstream unmaintained crates already tracked in [dependency review](dependency-review.md). Environment-dependent Secret Service tests remain ignored. No live mailbox content is used.
- No live Entra sign-in or Graph mailbox request has been performed. The candidate does not claim tenant verification.
- The M3 calendar code has not yet been ported onto the current M2 base. No calendar Graph request or desktop window has been used.

## Active risks

- No dedicated Microsoft 365 test tenant or public app registration is available. Keep issue #3 open and leave issue #8's tenant-only sync evidence unverified.
- Issue #9 remains open for real-window layout, operating-system clipboard behavior, and system-browser launch. Do not launch a GUI during this work.
- Issue #22 still needs current-head integration and a safe manual screen-reader, focus, scrolling, and layout review. Keep issue #6 open for live recurrence and all-day verification.
- Fedora GNOME Keyring evidence does not establish behavior for every Secret Service implementation.
- Windows and macOS build, runtime, credential storage, installation, and operating-system integration remain deferred.

## Next actions

1. Keep PR #47 in draft until issue #8's Microsoft 365 initial-sync check and issue #9's real-window checks pass.
2. Keep issue #8 open until initial-sync completeness is verified against a Microsoft 365 test tenant.
3. Keep issue #9 open until layout, clipboard behavior, and browser launch are verified in a safe Linux session.
4. Port issue #22 onto the current M2 session and cache architecture, then run its Linux checks without launching the desktop.

## Update rule

Update this file when the milestone, an active risk, or a next action changes. Keep implementation plans and acceptance criteria in GitHub issues.
