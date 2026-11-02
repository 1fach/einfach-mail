# Project status

Last updated: 2026-11-01

## Current milestone

M1 identity and local-data proofs. Linux is the sole development and validation target. The GPUI Kit foundation and Fedora shell launch are complete. The Linux account connection, identity display, and permission renewal are implemented in the local M1 candidate. [Issue #45](https://github.com/1fach/einfach-mail/issues/45) adds cancellation ordering across connection and renewal credential commits. Synthetic provider, workflow, worker, and headless GPUI tests cover cancellation before and during each commit, retry, and restore through a new provider. The documented Linux gate passed locally for this candidate on 2026-11-01. Live Entra evidence remains open under issue #3 because no dedicated test tenant or public app registration is available. Production mail, calendar, and synchronization workflows are not included in this candidate. Windows and macOS work is deferred under [ADR 0010](../decisions/0010-linux-first-development.md).

## Verified state

- The foundation is on `main` as one commit above the empty root. PR #13 was rebase-merged. Linux-only CI and the Linux credential-store proof were merged in PR #21, closing issue #20.
- The account view uses an application-owned workflow and a provider-neutral account profile. The Microsoft provider uses OAuth authorization code with PKCE, Graph `/me`, and Linux Secret Service for refresh credentials. Synthetic provider and headless UI tests cover the account flow.
- Issue #45's local implementation orders cancellation against refresh-credential persistence. If cancellation wins first, the operation remains retryable and preserves an existing credential. If credential commit starts first, the view keeps the task alive and shows its actual connected or failed result. Permission renewal keeps the active account and requests only its existing `User.Read offline_access` scopes. The candidate contains no mail, calendar, or synchronization implementation.
- The candidate passed Rust 1.95 workspace compilation, formatting, Clippy with warnings denied, documentation generation, dependency-boundary and Tauri/WebView checks, RustSec audit, and synthetic workspace tests on 2026-11-01. The audit found no known vulnerable or yanked dependencies; it reported eight existing transitive unmaintained-crate notices listed in [dependency review](dependency-review.md).
- The Linux credential proof covers Fedora GNOME Keyring with the service unlocked. It uses synthetic credentials and does not access a real mailbox.
- No live Graph request or Entra sign-in has been performed. Source review and synthetic fixtures do not prove browser, callback, MFA, Conditional Access, consent renewal or revocation, refresh, or sign-out behavior against a tenant.

## Active risks

- No dedicated Microsoft 365 test tenant or public app registration is available. Keep issue #3 open until its live evidence is complete.
- The registered callback port, default-browser launch, and Entra consent behavior remain unverified against a tenant.
- Fedora GNOME Keyring results do not establish behavior across every Secret Service implementation.
- Windows and macOS build, runtime, credential storage, installation, and operating-system integration remain deferred.

## Next actions

1. Keep issue #45 open for publication and review of the locally verified M1 candidate.
2. Keep issue #3 open until a dedicated tenant and public app registration are available for the opt-in live proof.
3. Continue M2 only after the M1 account and storage gates are reviewed.

## Update rule

Update this file when the milestone, an active risk, or a next action changes. Keep implementation plans and acceptance criteria in GitHub issues.
