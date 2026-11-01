# Project status

Last updated: 2026-11-05

## Current milestone

M2 read-only mail remains in draft review in PR #47. Its current head, `c09ccdd92fc4597b4130e86a957b28f7fdac624f`, passes all four required GitHub checks. The reviewed follow-up adds shared account and folder reservations, persisted checkpoint revisions, and synthetic concurrency regressions for stale responses. The full Linux project gate passed locally on 2026-11-04, including workspace tests, Clippy, rustdoc, RustSec audit, and dependency-policy checks. Formatting, all 18 desktop tests, and workspace rustdoc passed again after the final copy and Rustdoc edits. The lockfile uses upstream-fixed `async-executor` 1.14.1. No live Entra or Graph request has been made.

M3 read-only calendar integration is ready for stacked draft review on clean branch `candidate/issue-22-calendar-sync`, based on PR #47 head `c09ccdd92fc4597b4130e86a957b28f7fdac624f`. The full Linux gate and source review passed on 2026-11-04. Workspace tests passed with 181 tests passing and 3 environment-dependent tests ignored. RustSec found no vulnerable or yanked locked crates and reported 8 allowed unmaintained-crate advisories. ADRs 0014 and 0038 record the calendar scope and shared per-account session boundary. No calendar window or live Graph request has been used.

Issue #44's retryable cache-removal fix passed source review and the full Linux gate on 2026-11-05 in isolated branch `candidate/issue-44-retryable-cache-removal`. The fix now treats filesystem inspection failures as errors before creating or deleting a cache key, and the retry guidance covers both cache-file access and the Linux keyring. Synthetic regression tests cover inaccessible cache paths and the recovery message; existing tests cover SQLCipher recovery after sidecar failures and retry after provider credential deletion fails. RustSec reported no vulnerable or yanked locked crates and 8 allowed unmaintained-crate advisories. Three environment-dependent Secret Service tests remain ignored; no real account, live Graph request, or GUI was used. Keep Issue #44 open and PR #52 in draft until review and merge.

Issue #23's read-state implementation is isolated on `candidate/issue-23-read-state-pr`, based on PR #52's head and alongside the send draft in PR #53. Draft PR #54 is open for review. The implementation and source review are complete. `bash scripts/check-linux-local.sh` passed on 2026-11-04. Issue #31's follow-up flag implementation is isolated on `candidate/issue-31-flags-pr` and stacked on PR #54's current head, `19c41468b139cac4facc8e67b14ecaa93d1f46d9`, in draft PR #55. After restacking, `bash scripts/check-linux-local.sh` passed on 2026-11-05. Synthetic tests cover Graph parsing and PATCH responses, SQLCipher migration and recovery, and headless GPUI feedback. Three Secret Service tests remain ignored because they need a desktop service. No test tenant, real mailbox, or GUI was used.

Issue #27's M4 plain-text send implementation is in draft PR #53. Its head is `candidate/issue-27-plain-text-send`. Its base is PR #52's `candidate/issue-44-retryable-cache-removal` branch. The local Linux gate passed, including Rust 1.95 all-target compilation, Clippy, rustdoc, dependency policy, RustSec audit, formatting, and workspace tests. Three environment-dependent Secret Service tests remain ignored. Cache staging and removal serialize across app processes. Stale cache handles reject sends after database removal, and lock waits run off the async executor. No live Graph request, real mailbox, or desktop window was used. Tenant verification remains under issue #28. Keep issue #27 open until review and merge.

Issue #23's read-state action is in draft PR #54, stacked on PR #53. Its Linux checks passed before this restack; final-stack verification is pending. The implementation uses synthetic tests only. No live Graph request, real mailbox, or desktop window was used. Keep issue #23 open through review and issue #24 open for tenant verification.

Issue #29's reply implementation is in draft PR #56, stacked on PR #55. It covers the native composer, cached-source checks, Graph POST mapping, and body-free restart recovery. The branch is being restacked to remove an older duplicate flags commit. Final Linux verification remains pending. Keep issue #30 open for tenant-only consent, `Reply-To`, and Sent Items evidence.

PR #46 merged into `main` on 2026-11-02. It contains the M1 account connection and cancellation boundary. Keep issue #3 open until live Entra checks pass.

## Verified state

- The Linux GPUI Kit foundation and encrypted-cache/Secret Service proof are merged. Linux is the current development and validation target; Windows and macOS support are deferred under [ADR 0010](../decisions/0010-linux-first-development.md).
- The account view uses an application workflow and provider-neutral profile. The Microsoft provider uses authorization code with PKCE, Graph `/me`, and Linux Secret Service for refresh credentials. Cancellation is ordered against credential commits so the view reports the operation's actual outcome.
- The M2 candidate in PR #47 contains read-only folder and message synchronization, a fixed 90-day cutoff, encrypted transactions that commit each page and cursor together, fetch-on-open bodies, offline cached browsing, keyboard navigation, and native rendering of sanitized message content. Expired message delta cursors start a durable replacement scan while cached messages remain visible. The M2 candidate contains no calendar workflows or mail-write actions.
- Sessions created by one mail workflow factory share account and folder reservations. A sync reserves the account before folder enumeration and holds it through folder replacement and delta sync. Each persisted page or reset advances a checkpoint revision, and stale responses cannot advance or finalize a newer synchronization state.
- Automated fixtures use synthetic data. The candidate includes Graph transport, persistence, sanitizer, and headless GPUI proof packages. The mail fixes pass the Rust 1.95 workspace/all-target check, full workspace tests, affected-crate Clippy, rustdoc, and formatting. Dependency-boundary and banned-framework checks pass. After CI identified yanked `async-executor` 1.14.0, the lockfile was updated to upstream-fixed 1.14.1; a fresh RustSec audit loaded 1,403 advisories and found no vulnerable or yanked locked crates, while reporting the eight upstream unmaintained crates already tracked in [dependency review](dependency-review.md). Environment-dependent Secret Service tests remain ignored. No live mailbox content is used.
- No live Entra sign-in or Graph mailbox request has been performed. The candidate does not claim tenant verification.
- The M3 port adds account-scoped calendar workflows, Graph calendar-view delta paging, range-scoped SQLCipher caching, and a native weekly GPUI agenda. The full Linux gate and source review pass locally. Synthetic tests cover the request, workflow, persistence, and headless agenda paths. No calendar Graph request or desktop window has been used.
- Issue #23 adds one M4 action: users can set a cached message read or unread after Graph confirms the change. The encrypted cache updates only after confirmation. An uncertain result blocks more read-state writes until a successful account sync. The permission review discloses the full delegated `Mail.ReadWrite` capability. Tenant consent and live Graph behavior remain unverified under Issue #24.
- The M4 send candidate adds a native single-recipient plain-text composer, same-account delegated `Mail.Send` renewal, a one-shot Graph `/me/sendMail` request, and an encrypted body-free marker that blocks replay and account removal while unresolved. Confirmed acceptance, definitive rejection, and uncertain outcomes have separate recovery paths; fixtures cover redirect, throttle, transport failure, restart, cache failure, duplicate-send prevention, and cross-process cache lifecycle races. Live permission consent and Graph behavior are unverified under issue #28.

## Active risks

- No dedicated Microsoft 365 test tenant or public app registration is available. Keep issue #3 open and leave issue #8's tenant-only sync evidence unverified.
- Keep Issue #24 open until a dedicated tenant verifies `Mail.ReadWrite` consent and read-state PATCH behavior.
- Keep issue #31 open through review. Keep issue #32 open until a dedicated tenant verifies follow-up flag delta and PATCH behavior.
- Issue #9 remains open for real-window layout, operating-system clipboard behavior, and system-browser launch. Do not launch a GUI during this work.
- The Issue #22 candidate is ready for stacked review on `candidate/issue-22-calendar-sync`, based on PR #47. Keep issue #22 open for manual Linux review of screen-reader announcements, focus, scrolling, and layout. Keep issue #6 open for live recurrence, time-zone, and all-day verification.
- Draft PR #53 implements Issue #27 and is stacked on PR #52. Keep issue #27 open until review and merge, and keep issue #28 open for tenant-only consent and Graph verification.
- Issue #29 has local synthetic coverage but no review branch yet. Keep issue #30 open for tenant-only reply verification.
- Fedora GNOME Keyring evidence does not establish behavior for every Secret Service implementation.
- Windows and macOS build, runtime, credential storage, installation, and operating-system integration remain deferred.

## Next actions

1. Finish local Linux verification of the reconciled M4 send, read-state, flags, reply, move, and account-removal stack before publishing branch updates.
2. Update issue and PR evidence after the restacked branch heads are stable; keep draft states unchanged and do not merge.
3. Keep PR #47 in draft until issue #8's Microsoft 365 initial-sync check and issue #9's real-window checks pass.
4. Keep issue #8 open until initial-sync completeness is verified against a Microsoft 365 test tenant.
5. Keep issue #9 open until layout, clipboard behavior, and browser launch are verified in a safe Linux session.
6. Keep issue #22 open until the manual Linux review of screen-reader announcements, focus, scrolling, and layout is complete. Keep issue #6 open until live Graph recurrence, time-zone, and all-day behavior is verified.
7. Keep issues #24, #28, #30, and #32 open for tenant-only consent and Graph verification.
8. After PR #50 merges, review the independent M3 follow-ups in PRs #51 and #52 for merge.

## Update rule

Update this file when the milestone, an active risk, or a next action changes. Keep implementation plans and acceptance criteria in GitHub issues.
