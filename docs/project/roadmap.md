# Roadmap

Milestones describe product outcomes. Track active implementation tasks and their acceptance criteria in GitHub issues. Use [project status](status.md) for the current milestone and immediate next actions. Update this roadmap when milestone scope or sequencing changes.

## M0: Repository foundation — in progress

Provide a reproducible Rust workspace, native GPUI Kit shell, contributor workflow, and cross-platform baseline.

Fedora startup is verified in [issue #1](https://github.com/1fach/einfach-mail/issues/1). The foundation snapshot passed all eight automated checks on [PR #13](https://github.com/1fach/einfach-mail/pull/13), which remains unmerged. Check the PR for the latest run. Review and land that foundation PR before M1. Keep [issue #10](https://github.com/1fach/einfach-mail/issues/10) open until the RustSec workflow passes on `main`. Keep [issue #2](https://github.com/1fach/einfach-mail/issues/2) open for an interactive Windows launch. Complete it before closing M0. M1 can start after the foundation lands and the main-branch audit passes, with issue #2 still open.

## M1: Identity and local data — not started

Connect a supported Microsoft account with delegated access and prove that credentials and cached data stay within their approved security boundaries. Authentication and encrypted-storage decisions remain subject to the proofs in ADRs [0003](../decisions/0003-desktop-authentication.md), [0005](../decisions/0005-encrypted-local-cache.md), and [0008](../decisions/0008-identity-token-boundary.md).

M1 work is grouped under the [GitHub milestone](https://github.com/1fach/einfach-mail/milestone/2): [prove delegated sign-in](https://github.com/1fach/einfach-mail/issues/3), [prove encrypted storage](https://github.com/1fach/einfach-mail/issues/4), and [connect an account and display its identity](https://github.com/1fach/einfach-mail/issues/5). Start issues #3 and #4 after the foundation lands and the main-branch audit passes. Both issues still require Windows evidence. Start issue #5 after the authentication proof in issue #3 passes. Issue #2 tracks the separate interactive shell launch and may remain open while M1 starts.

## M2: Read-only mail — not started

Let the user browse folders, read synchronized messages from the offline cache, and view message content safely. M2 does not change mail on the server; mail and event actions belong to M4. The Graph transport and message renderer remain subject to ADRs [0006](../decisions/0006-graph-http-transport.md) and [0007](../decisions/0007-message-body-rendering.md).

M2 work is grouped under the [GitHub milestone](https://github.com/1fach/einfach-mail/milestone/4): [prove the Graph mail transport and delta contracts](https://github.com/1fach/einfach-mail/issues/7), [browse folders and read synchronized mail offline](https://github.com/1fach/einfach-mail/issues/8), and [render cached message bodies safely in native GPUI](https://github.com/1fach/einfach-mail/issues/9). The workflow and rendering depend on M1 identity and encrypted-cache proofs.

## M3: Read-only calendar — not started

Show and synchronize calendar events with correct time-zone and recurrence behavior.

The [M3 GitHub milestone](https://github.com/1fach/einfach-mail/milestone/3) tracks this work. Resolve the Graph time-zone mapping and recurrence design gate in [issue #6](https://github.com/1fach/einfach-mail/issues/6) before calendar implementation.

## M4: Mail and calendar actions — not started

Support the first defined mail and event changes with clear pending, confirmed, and failed states.

## M5: Windows daily-use integration — not started

Add and verify the Windows integrations that improve the core mail and calendar workflow, accessibility, installation, and account removal.

## M6: Broader release targets — not started

Evaluate additional operating systems, account types, and release policies after the Windows workflow is reliable.
