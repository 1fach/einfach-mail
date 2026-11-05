# Project status

Last updated: 2026-11-05

## Current milestone

M2 read-only mail remains in draft review in PR #47. Its current head, `c09ccdd92fc4597b4130e86a957b28f7fdac624f`, passes all four required GitHub checks. The reviewed follow-up adds shared account and folder reservations, persisted checkpoint revisions, and synthetic concurrency regressions for stale responses. The full Linux project gate passed locally on 2026-11-04, including workspace tests, Clippy, rustdoc, RustSec audit, and dependency-policy checks. Formatting, all 18 desktop tests, and workspace rustdoc passed again after the final copy and Rustdoc edits. The lockfile uses upstream-fixed `async-executor` 1.14.1. No live Entra or Graph request has been made.

M3 read-only calendar integration remains in draft PR #50 on branch `candidate/issue-22-calendar-sync`, based on PR #47 head `c09ccdd92fc4597b4130e86a957b28f7fdac624f`. On 2026-11-05, the native agenda gained one listbox focus stop, no-wrap Up and Down navigation, Home and End selection, selected-option accessibility state, and scroll-to-selection behavior. The headless test covers focus, key boundaries, click-to-focus, and selected-row visibility. All checks in `scripts/check-linux-local.sh` passed locally. RustSec loaded 1,409 advisories and found no vulnerable or yanked locked crates; eight allowed unmaintained-crate advisories remain. ADRs 0014, 0038, and 0039 record the calendar scope, shared per-account session, and agenda interaction. Manual Linux screen-reader, focus, scrolling, and layout review remains open. No calendar window or live Graph request has been used.

Issue #44's retryable cache-removal change passed source review and the full Linux gate on 2026-11-05. It retains shared per-account locks for open cache sessions, serializes first-time key setup and migrations, and rejects removal while another session remains open. The account screen keeps long recovery states scrollable so the retry action remains reachable. PR #52 merged into PR #50's branch on 2026-11-05 as merge commit `ffe4755ce4ce1b42529dbd37a56254e141f855c9`; all four required GitHub checks passed before merge. Keep Issue #44 open until the stacked changes reach `main`. Three environment-dependent Secret Service tests remain ignored; no real account, live Graph request, or GUI was used.

PR #46 merged into `main` on 2026-11-02. It contains the M1 account connection and cancellation boundary. Keep issue #3 open until live Entra checks pass.

## Verified state

- The Linux GPUI Kit foundation and encrypted-cache/Secret Service proof are merged. Linux is the current development and validation target; Windows and macOS support are deferred under [ADR 0010](../decisions/0010-linux-first-development.md).
- The account view uses an application workflow and provider-neutral profile. The Microsoft provider uses authorization code with PKCE, Graph `/me`, and Linux Secret Service for refresh credentials. Cancellation is ordered against credential commits so the view reports the operation's actual outcome.
- The M2 candidate contains read-only folder and message synchronization, a fixed 90-day cutoff, encrypted transactions that commit each page and cursor together, fetch-on-open bodies, offline cached browsing, keyboard navigation, and native rendering of sanitized message content. Expired message delta cursors start a durable replacement scan while cached messages remain visible. The candidate contains no calendar workflows or mail-write actions.
- Sessions created by one mail workflow factory share account and folder reservations. A sync reserves the account before folder enumeration and holds it through folder replacement and delta sync. Each persisted page or reset advances a checkpoint revision, and stale responses cannot advance or finalize a newer synchronization state.
- Automated fixtures use synthetic data. The candidate includes Graph transport, persistence, sanitizer, and headless GPUI proof packages. The mail fixes pass the Rust 1.95 workspace/all-target check, full workspace tests, affected-crate Clippy, rustdoc, and formatting. Dependency-boundary and banned-framework checks pass. After CI identified yanked `async-executor` 1.14.0, the lockfile was updated to upstream-fixed 1.14.1; a fresh RustSec audit loaded 1,403 advisories and found no vulnerable or yanked locked crates, while reporting the eight upstream unmaintained crates already tracked in [dependency review](dependency-review.md). Environment-dependent Secret Service tests remain ignored. No live mailbox content is used.
- No live Entra sign-in or Graph mailbox request has been performed. The candidate does not claim tenant verification.
- The M3 port adds account-scoped calendar workflows, Graph calendar-view delta paging, range-scoped SQLCipher caching, and a native weekly GPUI agenda. Synthetic tests cover the request, workflow, persistence, and headless agenda paths. No calendar Graph request or desktop window has been used.

## Active risks

- No dedicated Microsoft 365 test tenant or public app registration is available. Keep issue #3 open and leave issue #8's tenant-only sync evidence unverified.
- Issue #9 remains open for real-window layout, operating-system clipboard behavior, and system-browser launch. Do not launch a GUI during this work.
- PR #50 remains a draft stacked on PR #47. Keep issue #22 open for manual Linux review of screen-reader announcements, focus, scrolling, and layout. Keep issue #6 open for live recurrence, time-zone, and all-day verification.
- Fedora GNOME Keyring evidence does not establish behavior for every Secret Service implementation.
- Windows and macOS build, runtime, credential storage, installation, and operating-system integration remain deferred.

## Next actions

1. Keep PR #47 in draft until issue #8's Microsoft 365 initial-sync check and issue #9's real-window checks pass.
2. Keep issue #8 open until initial-sync completeness is verified against a Microsoft 365 test tenant.
3. Keep issue #9 open until layout, clipboard behavior, and browser launch are verified in a safe Linux session.
4. Keep PR #50 in draft and issue #22 open until PR #47 is eligible and the manual Linux review of screen-reader announcements, focus, scrolling, and layout is complete. Keep issue #6 open until live Graph recurrence, time-zone, and all-day behavior is verified.
5. Keep Issue #44 open until PR #50 reaches `main`; then review PR #51.

## Update rule

Update this file when the milestone, an active risk, or a next action changes. Keep implementation plans and acceptance criteria in GitHub issues.
