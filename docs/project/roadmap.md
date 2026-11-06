# Roadmap

Milestones describe product outcomes. Track active implementation tasks and their acceptance criteria in GitHub issues. Use [project status](status.md) for the current milestone and immediate next actions. Update this roadmap when milestone scope or sequencing changes.

## M0: Repository foundation — complete for Linux

Provide a reproducible Rust workspace, native GPUI Kit shell, contributor workflow, and Linux development baseline.

Fedora startup is verified in [issue #1](https://github.com/1fach/einfach-mail/issues/1). Foundation [PR #13](https://github.com/1fach/einfach-mail/pull/13) was rebase-merged on 2026-10-31 as the single commit above the empty root. The historical Windows CI check passed in the post-merge run, but current development CI targets Linux only. The interactive Windows shell launch is deferred to [M5 issue #2](https://github.com/1fach/einfach-mail/issues/2); see [ADR 0010](../decisions/0010-linux-first-development.md).

## M1: Identity and local data — in progress

Connect a supported Microsoft account with delegated access and prove that credentials and cached data stay within their approved security boundaries. The authentication and storage choices are recorded in ADRs [0003](../decisions/0003-desktop-authentication.md), [0005](../decisions/0005-encrypted-local-cache.md), and [0008](../decisions/0008-identity-token-boundary.md).

M1 work is grouped under the [GitHub milestone](https://github.com/1fach/einfach-mail/milestone/2): [prove delegated sign-in](https://github.com/1fach/einfach-mail/issues/3), [connect an account and display its identity](https://github.com/1fach/einfach-mail/issues/5), and [prove Linux credential storage](https://github.com/1fach/einfach-mail/issues/20). Linux account connection and identity display now have synthetic provider and headless UI coverage. Issue #5 is closed after the local Linux gate passed. PR #21 merged the Linux credential proof and closed issue #20. Keep issue #3 open for live system-browser, callback, MFA, Conditional Access, refresh, consent-revocation, and sign-out evidence. No dedicated test tenant or public app registration is available.

[Issue #45](https://github.com/1fach/einfach-mail/issues/45) tracks accurate cancellation state across account connection and permission renewal. The implementation is recorded in [ADR 0030](../decisions/0030-account-operation-cancellation-boundary.md). PR #46 merged into `main` on 2026-11-02 after its four required GitHub checks passed. Live tenant behavior remains under issue #3.

[Issue #48](https://github.com/1fach/einfach-mail/issues/48) binds saved sign-in credentials to the Graph-verified cache identity. [PR #63](https://github.com/1fach/einfach-mail/pull/63) merged the implementation into parent PR #54 at `cdb4951c7ad267d77b056e1b57169a1a62549c4b` on 2026-11-06, closing issue #48. Exact-head review found no actionable issues, and all four required GitHub checks passed on head `66c489142ba4bbb8475a58b36021070d6eaf7723`. [ADR 0041](../decisions/0041-bind-saved-sign-in-to-cache-identity.md) records the pending identity state, legacy credential handling, and cache-before-credential removal order. Synthetic Linux verification does not replace live Entra or Secret Service evidence; keep issue #3 open for tenant verification.

## M2: Read-only mail — in progress

Let the user browse folders, read synchronized messages from the offline cache, and view message content safely. M2 does not change mail on the server; mail and event actions belong to M4. The Graph transport and message renderer remain subject to ADRs [0006](../decisions/0006-graph-http-transport.md) and [0007](../decisions/0007-message-body-rendering.md).

M2 work is grouped under the [GitHub milestone](https://github.com/1fach/einfach-mail/milestone/4): [prove the Graph mail transport and delta contracts](https://github.com/1fach/einfach-mail/issues/7), [browse folders and read synchronized mail offline](https://github.com/1fach/einfach-mail/issues/8), and [render cached message bodies safely in native GPUI](https://github.com/1fach/einfach-mail/issues/9). Issue #7's synthetic Linux transport evidence is complete. The local candidate on `candidate/m2-read-only-mail` contains the production read-only sync, SQLCipher cache, accessible message list, sanitized renderer, and expired-cursor recovery. Its combined local Linux gate and source review passed on 2026-11-02. Live mailbox completeness for issue #8 and real-window rendering checks for issue #9 remain open. The candidate is based on `origin/main` and does not include calendar or mail-write features.

## M3: Read-only calendar — in progress

Show and synchronize calendar events with correct time-zone and recurrence behavior. The [M3 GitHub milestone](https://github.com/1fach/einfach-mail/milestone/3) tracks this work. Keep [issue #6](https://github.com/1fach/einfach-mail/issues/6) open for live recurrence, time-zone, and all-day verification.

The accepted scope is in [ADR 0014](../decisions/0014-linux-read-only-calendar-workflow.md). The [Issue #22](https://github.com/1fach/einfach-mail/issues/22) candidate is ready for stacked review on a clean branch based on PR #47's current head. The full local Linux gate and source review pass. No live tenant or manual desktop review is claimed.

## M4: Mail and calendar actions — in progress

Support explicit mail and event changes with clear pending, confirmed, and failed states. [ADR 0015](../decisions/0015-idempotent-message-read-state-update.md) defines read-state updates, and [ADR 0019](../decisions/0019-follow-up-flags-on-cached-messages.md) defines cached-message follow-up flags. Draft [PR #54](https://github.com/1fach/einfach-mail/pull/54) presents [issue #23](https://github.com/1fach/einfach-mail/issues/23) after PR #53. Its child [PR #55](https://github.com/1fach/einfach-mail/pull/55) merged the implementation for [issue #31](https://github.com/1fach/einfach-mail/issues/31) into PR #54 at `3bbd656ba2368cc9806974492cd0523453b4ec1f` on 2026-11-06, and issue #31 is closed. Keep issue #23 open through review and keep [issue #24](https://github.com/1fach/einfach-mail/issues/24) and [issue #32](https://github.com/1fach/einfach-mail/issues/32) open for delegated permission and Graph write verification in a dedicated test tenant.

[Issue #27](https://github.com/1fach/einfach-mail/issues/27) implements the first M4 action: composing and sending one plain-text message through Graph with delegated Mail.Send. Draft [PR #53](https://github.com/1fach/einfach-mail/pull/53) is stacked on PR #52. Live consent and Graph verification remain separate under [issue #28](https://github.com/1fach/einfach-mail/issues/28).

[Issue #29](https://github.com/1fach/einfach-mail/issues/29) adds a native plain-text reply to one cached message. PR #56 merged into PR #55 at `6ef2dd9c63ebd176f256a5c5f09eac99ddf99751` on 2026-11-06, and issue #29 is closed. The reply uses Graph's source-message routing and shares the send marker and no-retry recovery path in [ADR 0018](../decisions/0018-native-plain-text-message-reply.md). Tenant verification remains in [issue #30](https://github.com/1fach/einfach-mail/issues/30).

[Issue #33](https://github.com/1fach/einfach-mail/issues/33) adds a native action to move one cached message into a visible folder. PR #57 merged into PR #56 at `1f8588607809bb18fdacd398a8751262b0a8c377` on 2026-11-06, and issue #33 is closed. [ADR 0020](../decisions/0020-move-cached-messages-between-folders.md) records the Graph and cache behavior. Keep [issue #34](https://github.com/1fach/einfach-mail/issues/34) open for tenant-only move and immutable-ID verification.

[Issue #35](https://github.com/1fach/einfach-mail/issues/35) adds a native Delete action that moves an opened cached message to Deleted Items and preserves its encrypted body. [PR #62](https://github.com/1fach/einfach-mail/pull/62) merged into PR #61 at `95265f230b2314e7effd17dbbf50e11b435590ec`; issue #35 is closed. Its design is recorded in [ADR 0039](../decisions/0039-move-messages-to-deleted-items.md). [Issue #36](https://github.com/1fach/einfach-mail/issues/36) remains open for delegated consent and tenant-only Graph verification.

[Issue #37](https://github.com/1fach/einfach-mail/issues/37) adds a native Archive action for cached messages. [PR #64](https://github.com/1fach/einfach-mail/pull/64) merged into PR #62 at `7b51c931b0f1492744f381489d5ffa7c9f0293e5`, and that stack merged into PR #61 at `95265f230b2314e7effd17dbbf50e11b435590ec`; issue #37 is closed. [ADR 0040](../decisions/0040-move-messages-to-the-default-archive-folder.md) records the shared typed workflow and default Archive folder behavior. Keep issue #38 open for tenant verification.

[Issue #43](https://github.com/1fach/einfach-mail/issues/43) serializes account removal with active mail operations and limits the Linux desktop to one process per user. [PR #58](https://github.com/1fach/einfach-mail/pull/58) merged into PR #57 at `3923a810b13692ff09006a0a1d153a2f54772bae` on 2026-11-06, closing issue #43. The concurrency and process boundaries are recorded in [ADRs 0027](../decisions/0027-serialize-mail-account-removal-with-mail-operations.md) and [0028](../decisions/0028-single-linux-desktop-process.md). Verification is local and synthetic.

[Issues #25](https://github.com/1fach/einfach-mail/issues/25), [#39](https://github.com/1fach/einfach-mail/issues/39), and [#41](https://github.com/1fach/einfach-mail/issues/41) add creation, narrow editing, and confirmed deletion of standalone timed appointments. PR #59 merged into candidate [PR #58](https://github.com/1fach/einfach-mail/pull/58) at `333707c4e1eefbd96e83cfcde7948d58ef913ef3`; issues #25, #39, and #41 are closed. The designs are recorded in [ADR 0016](../decisions/0016-idempotent-primary-calendar-event-creation.md), [ADR 0023](../decisions/0023-edit-standalone-calendar-appointments.md), and [ADR 0026](../decisions/0026-delete-standalone-calendar-appointments.md). PR #59 also carried the message Delete and Archive work from PRs #62 and #64; issues #35 and #37 are closed. Review found that creation bypassed ADR 0026's initial-sync requirement. The workflow and native UI now gate every event write on a successful target-view sync and block writes until account-wide recovery finishes. Initial-sync and account-recovery regressions pass locally, along with the complete Linux project gate and all four required PR #59 checks. PR #58 merged into PR #57 at `3923a810b13692ff09006a0a1d153a2f54772bae`, then PR #57 merged into PR #56 at `1f8588607809bb18fdacd398a8751262b0a8c377`; issue #33 is closed. Keep issues #26, #40, and #42 open for tenant verification. The update flow retains a documented final-read-to-PATCH race; tenant verification for DELETE's documented ETag precondition remains open.

## M5: Windows support and daily-use integration — not started

Add and verify Windows shell launch, authentication and credential storage, accessibility, installation, account removal, and operating-system integrations for mail and calendar use.

## M6: macOS and broader release targets — not started

Add and verify macOS support, then evaluate other account types and release policies after the core workflow is reliable.
