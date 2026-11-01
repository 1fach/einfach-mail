# Roadmap

Milestones describe product outcomes. Track active implementation tasks and their acceptance criteria in GitHub issues. Use [project status](status.md) for the current milestone and immediate next actions. Update this roadmap when milestone scope or sequencing changes.

## M0: Repository foundation — complete for Linux

Provide a reproducible Rust workspace, native GPUI Kit shell, contributor workflow, and Linux development baseline.

Fedora startup is verified in [issue #1](https://github.com/1fach/einfach-mail/issues/1). Foundation [PR #13](https://github.com/1fach/einfach-mail/pull/13) was rebase-merged on 2026-10-31 as the single commit above the empty root. The historical Windows CI check passed in the post-merge run, but current development CI targets Linux only. The interactive Windows shell launch is deferred to [M5 issue #2](https://github.com/1fach/einfach-mail/issues/2); see [ADR 0010](../decisions/0010-linux-first-development.md).

## M1: Identity and local data — in progress

Connect a supported Microsoft account with delegated access and prove that credentials and cached data stay within their approved security boundaries. The authentication and storage choices are recorded in ADRs [0003](../decisions/0003-desktop-authentication.md), [0005](../decisions/0005-encrypted-local-cache.md), and [0008](../decisions/0008-identity-token-boundary.md).

M1 work is grouped under the [GitHub milestone](https://github.com/1fach/einfach-mail/milestone/2): [prove delegated sign-in](https://github.com/1fach/einfach-mail/issues/3), [connect an account and display its identity](https://github.com/1fach/einfach-mail/issues/5), and [prove Linux credential storage](https://github.com/1fach/einfach-mail/issues/20). Linux account connection and identity display now have synthetic provider and headless UI coverage. Issue #5 is closed after the local Linux gate passed. PR #21 merged the Linux credential proof and closed issue #20. Keep issue #3 open for live system-browser, callback, MFA, Conditional Access, refresh, consent-revocation, and sign-out evidence. No dedicated test tenant or public app registration is available.

[Issue #45](https://github.com/1fach/einfach-mail/issues/45) tracks accurate cancellation state across account connection and permission renewal. The implementation is recorded in [ADR 0030](../decisions/0030-account-operation-cancellation-boundary.md). PR #46 contains the M1 candidate; its four required GitHub checks passed, and it remains open and unmerged. Live tenant behavior remains under issue #3.

## M2: Read-only mail — in progress

Let the user browse folders, read synchronized messages from the offline cache, and view message content safely. M2 does not change mail on the server; mail and event actions belong to M4. The Graph transport and message renderer remain subject to ADRs [0006](../decisions/0006-graph-http-transport.md) and [0007](../decisions/0007-message-body-rendering.md).

M2 work is grouped under the [GitHub milestone](https://github.com/1fach/einfach-mail/milestone/4): [prove the Graph mail transport and delta contracts](https://github.com/1fach/einfach-mail/issues/7), [browse folders and read synchronized mail offline](https://github.com/1fach/einfach-mail/issues/8), and [render cached message bodies safely in native GPUI](https://github.com/1fach/einfach-mail/issues/9). Issue #7's synthetic Linux transport evidence is complete. The local candidate on `candidate/m2-read-only-mail` contains the production read-only sync, SQLCipher cache, accessible message list, and sanitizer/renderer work for issues #8 and #9. Its combined local Linux gate and source review passed on 2026-11-01. Live mailbox completeness for issue #8 and real-window rendering checks for issue #9 remain open. The candidate is stacked on unmerged PR #46 and does not include calendar or mail-write features.

## M3: Read-only calendar — not started

Show and synchronize calendar events with correct time-zone and recurrence behavior. The [M3 GitHub milestone](https://github.com/1fach/einfach-mail/milestone/3) tracks this work. Resolve the Graph time-zone and recurrence design gate in [issue #6](https://github.com/1fach/einfach-mail/issues/6) before implementation.

## M4: Mail and calendar actions — not started

Support the first defined mail and event changes with clear pending, confirmed, and failed states.

## M5: Windows support and daily-use integration — not started

Add and verify Windows shell launch, authentication and credential storage, accessibility, installation, account removal, and operating-system integrations for mail and calendar use.

## M6: macOS and broader release targets — not started

Add and verify macOS support, then evaluate other account types and release policies after the core workflow is reliable.
