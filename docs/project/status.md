# Project status

Last updated: 2026-11-06

## Current milestone

M2 read-only mail remains in draft review in PR #47. Its current head, `c09ccdd92fc4597b4130e86a957b28f7fdac624f`, passes all four required GitHub checks. The reviewed follow-up adds shared account and folder reservations, persisted checkpoint revisions, and synthetic concurrency regressions for stale responses. The full Linux project gate passed locally on 2026-11-04, including workspace tests, Clippy, rustdoc, RustSec audit, and dependency-policy checks. Formatting, all 18 desktop tests, and workspace rustdoc passed again after the final copy and Rustdoc edits. The lockfile uses upstream-fixed `async-executor` 1.14.1. No live Entra or Graph request has been made.

M3 read-only calendar integration is ready for stacked draft review on clean branch `candidate/issue-22-calendar-sync`, based on PR #47 head `c09ccdd92fc4597b4130e86a957b28f7fdac624f`. The full Linux gate and source review passed on 2026-11-04. Workspace tests passed with 181 tests passing and 3 environment-dependent tests ignored. RustSec found no vulnerable or yanked locked crates and reported 8 allowed unmaintained-crate advisories. ADRs 0014 and 0038 record the calendar scope and shared per-account session boundary. No calendar window or live Graph request has been used.

Issue #44's retryable cache-removal fix is in PR #52, stacked on PR #50. PR #52 also takes the per-account lock when the cache directory is absent, rechecks cache presence under the lock, and removes credentials only after cache cleanup succeeds. Synthetic tests cover concurrent cache opening and no-open removal. The complete Linux gate passed on the final #63 and #64 leaves. Keep issue #44 open and PR #52 in draft through review.

Issue #23 remains in draft PR #54 after plain-text send in PR #53. Issue #31's follow-up flag implementation merged into PR #54 through PR #55 at `3bbd656ba2368cc9806974492cd0523453b4ec1f` on 2026-11-06, and issue #31 is closed. Exact-head review found no blocking or important findings, and all four required checks passed. PR #63's saved identity migration is now part of PR #54; its updated head needs fresh checks before review. The work uses synthetic Graph, persistence, workflow, and headless GPUI coverage; no live tenant or GUI was used. Keep issues #24 and #32 open for tenant verification.

Issue #27's plain-text send implementation is in draft PR #53, based on PR #52. The fallible cache-presence result flows through account opening, send staging, and no-open sign-out. The complete Linux gate passed on both final leaf branches. Keep issue #28 open for tenant consent and Graph verification.

Issue #29's reply implementation merged through PR #56 into PR #55 at `6ef2dd9c63ebd176f256a5c5f09eac99ddf99751` on 2026-11-06, and issue #29 is closed. The exact-head review found no blocking issues; all four required checks passed. Synthetic Graph, workflow, persistence/restart, and headless GPUI coverage is in the stack. No live tenant or GUI was used. Keep issue #30 open for tenant-only consent, Reply-To, and Sent Items evidence.

Issue #33's move merged into PR #56 through PR #57 at `1f8588607809bb18fdacd398a8751262b0a8c377` on 2026-11-06, and issue #33 is closed. The exact-head review found no blocking issues, and all four required checks passed. The Move action stays disabled until sync clears the selected folder's write marker. PR #58's account-removal and single-process changes remain in the same stack. Keep issue #34 open for tenant-only move and immutable-ID verification.

PR #59 merged into PR #58 at `333707c4e1eefbd96e83cfcde7948d58ef913ef3` on 2026-11-06. Issues #25, #39, and #41 cover event creation, editing, and deletion; all three issues are closed. PR #59 also carries the message Delete and Archive work from PRs #62 and #64; issues #35 and #37 are closed. The full Linux gate and all four required GitHub checks passed on PR #59 head `617da967e17c5c6e0548351c71d16cc8ad4c937a`. PR #58 merged into PR #57 at `3923a810b13692ff09006a0a1d153a2f54772bae`, and PR #57 merged into PR #56 at `1f8588607809bb18fdacd398a8751262b0a8c377` on 2026-11-06.

Review found that event creation could stage a request before the first successful sync, contrary to ADR 0026. The workflow and native UI now require a successful sync of the target view, and all account recovery views must sync before another calendar write. Synthetic workflow and headless GPUI regressions cover initial-sync and account-recovery gating. Keep issues #26, #40, and #42 open for tenant verification. ADR 0026 records OData's weak-ETag comparison rule. The documented DELETE `If-Match` behavior remains unverified in Exchange Online. No tenant, live Graph request, real mailbox, or GUI was used.

Issue #48's saved sign-in identity migration merged into PR #54 through PR #63 at `cdb4951c7ad267d77b056e1b57169a1a62549c4b` on 2026-11-06, and issue #48 is closed. Exact-head review found no actionable issues, and all four required GitHub checks passed on PR #63 head `66c489142ba4bbb8475a58b36021070d6eaf7723`, based on PR #54 head `3bbd656ba2368cc9806974492cd0523453b4ec1f`. Synthetic coverage includes credential migration, pending identity across restart, mismatch rejection, replacement sign-in blocking, and cache-before-credential removal. No live Entra request, real mailbox, desktop Secret Service lifecycle, or native window was used. Live Entra and Secret Service evidence remain under issue #3.

PR #46 merged into `main` on 2026-11-02. It contains the M1 account connection and cancellation boundary. Keep issue #3 open until live Entra checks pass.

## Verified state

- The Linux GPUI Kit foundation and encrypted-cache/Secret Service proof are merged. Linux is the current development and validation target; Windows and macOS support are deferred under [ADR 0010](../decisions/0010-linux-first-development.md).
- The account view uses an application workflow and provider-neutral profile. The Microsoft provider uses authorization code with PKCE, Graph `/me`, and Linux Secret Service for refresh credentials. Cancellation is ordered against credential commits so the view reports the operation's actual outcome.
- The M2 candidate in PR #47 contains read-only folder and message synchronization, a fixed 90-day cutoff, encrypted transactions that commit each page and cursor together, fetch-on-open bodies, offline cached browsing, keyboard navigation, and native rendering of sanitized message content. Expired message delta cursors start a durable replacement scan while cached messages remain visible. The M2 candidate contains no calendar workflows or mail-write actions.
- Sessions created by one mail workflow factory share account and folder reservations. A sync reserves the account before folder enumeration and holds it through folder replacement and delta sync. Each persisted page or reset advances a checkpoint revision, and stale responses cannot advance or finalize a newer synchronization state.
- Automated fixtures use synthetic data. The candidate includes Graph transport, persistence, sanitizer, and headless GPUI proof packages. The mail fixes pass the Rust 1.95 workspace/all-target check, full workspace tests, Clippy, rustdoc, and formatting. Dependency-boundary and banned-framework checks pass. After CI identified yanked `async-executor` 1.14.0, the lockfile was updated to upstream-fixed 1.14.1. The current RustSec audit found no vulnerable or yanked locked crates and reported eight allowed unmaintained-crate advisories. Two Secret Service tests remain ignored. No live mailbox content is used.
- No live Entra sign-in or Graph mailbox request has been performed. The candidate does not claim tenant verification.
- The M3 port adds account-scoped calendar workflows, Graph calendar-view delta paging, range-scoped SQLCipher caching, and a native weekly GPUI agenda. The full Linux gate and source review pass locally. Synthetic tests cover the request, workflow, persistence, and headless agenda paths. No calendar Graph request or desktop window has been used.
- Issue #23 adds one M4 action: users can set a cached message read or unread after Graph confirms the change. The encrypted cache updates only after confirmation. An uncertain result blocks more read-state writes until a successful account sync. The permission review discloses the full delegated `Mail.ReadWrite` capability. Tenant consent and live Graph behavior remain unverified under Issue #24.
- The M4 send candidate adds a native single-recipient plain-text composer, same-account delegated `Mail.Send` renewal, a one-shot Graph `/me/sendMail` request, and an encrypted body-free marker that blocks replay and account removal while unresolved. Confirmed acceptance, definitive rejection, and uncertain outcomes have separate recovery paths; fixtures cover redirect, throttle, transport failure, restart, cache failure, duplicate-send prevention, and cross-process cache lifecycle races. Live permission consent and Graph behavior are unverified under issue #28.
- Issue #29 adds a native plain-text reply to a cached message. It shares the send marker and uncertain-outcome recovery path; reply bodies are not persisted.
- Issue #33 adds a native move action for cached messages. It updates the encrypted cache only after exact Graph confirmation and preserves the cached body during reconciliation.
- Issue #43 serializes cache removal with account, folder, and calendar work, preserves the durable pending-send guard, and prevents a second Linux desktop process from opening the same per-user cache.
- Calendar event writes now include creation, editing, and deletion for supported timed standalone appointments. Each write requires a successful sync of its target view in the current workflow; editing and deleting also validate current Graph metadata. Writes are not automatically retried. Confirmed results reconcile all applicable encrypted cache views atomically. A stale event, uncertain outcome, or failed cache commit gates later writes until each previously synchronized account view completes a sync. Every view snapshot reports whether its own sync or account recovery sync remains required. The delete uses the final read's ETag through `If-Match`; live service behavior remains unverified.
- Message moves, Archive, and Delete require a successful account sync in the current mail session. An uncertain or canceled action, or a cache failure, disables later folder actions until a successful sync. Archive moves the message to the resolved default Archive folder. Delete moves it to recoverable Deleted Items and does not permanently delete it.

## Active risks

- No dedicated Microsoft 365 test tenant or public app registration is available. Keep issue #3 open and leave issue #8's tenant-only sync evidence unverified.
- Keep issues #36 and #38 open for tenant verification of Deleted Items resolution, message moves, Archive behavior, and delta reconciliation.
- Keep issues #26, #40, and #42 open for tenant-only event create, update, and delete evidence. Microsoft's Graph v1.0 OpenAPI documents no conditional precondition for PATCH, leaving a narrow update race; Exchange Online behavior for the documented DELETE `If-Match` precondition still needs verification.
- Keep Issue #24 open until a dedicated tenant verifies `Mail.ReadWrite` consent and read-state PATCH behavior.
- Issue #31 is closed after its reviewed implementation merged into PR #54. Keep issue #32 open until a dedicated tenant verifies follow-up flag delta and PATCH behavior.
- Issue #9 remains open for real-window layout, operating-system clipboard behavior, and system-browser launch. Do not launch a GUI during this work.
- The Issue #22 candidate is ready for stacked review on `candidate/issue-22-calendar-sync`, based on PR #47. Keep issue #22 open for manual Linux review of screen-reader announcements, focus, scrolling, and layout. Keep issue #6 open for live recurrence, time-zone, and all-day verification.
- Draft PR #53 implements Issue #27 and is stacked on PR #52. Keep issue #27 open until review and merge, and keep issue #28 open for tenant-only consent and Graph verification.
- Issue #29 is closed after PR #56 merged into PR #55. Issue #31 is closed after PR #55 merged into PR #54. Issue #33 is closed after PR #57 merged into PR #56. Issue #43 is closed after PR #58 merged into PR #57. Issue #48 is closed after PR #63 merged into PR #54.
- Fedora GNOME Keyring evidence does not establish behavior for every Secret Service implementation.
- Windows and macOS build, runtime, credential storage, installation, and operating-system integration remain deferred.

## Next actions

1. Review PR #54 after its updated four required checks pass. Keep issue #3 open for live Entra and Secret Service verification.
2. Keep PR #47 in draft until issue #8's Microsoft 365 sync check and issue #9's real-window checks pass.
3. Keep issue #22 open until the manual Linux review of screen-reader announcements, focus, scrolling, and layout is complete. Keep issue #6 open until live Graph recurrence, time-zone, and all-day behavior is verified.
4. Keep issues #24, #28, #30, #32, #34, #36, #38, #40, and #42 open for tenant-only consent and Graph verification.
5. Review the independent M3 follow-ups after their declared parent PRs clear their external blockers.

## Update rule

Update this file when the milestone, an active risk, or a next action changes. Keep implementation plans and acceptance criteria in GitHub issues.
