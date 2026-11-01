# Project status

Last updated: 2026-11-01

## Current milestone

M1 identity and local-data proofs. Linux is the sole development and validation target. The GPUI Kit foundation and Fedora shell launch are complete. The Linux account connection and identity display are implemented with synthetic provider coverage. Issue #45's cancellation guarantee is the next account-flow review item. Live Entra evidence remains open under issue #3 because no dedicated test tenant or public app registration is available. Production mail, calendar, and synchronization workflows are not included in this candidate. Windows and macOS work is deferred under [ADR 0010](../decisions/0010-linux-first-development.md).

## Verified state

- The foundation is on `main` as one commit above the empty root. PR #13 was rebase-merged. Linux-only CI and the Linux credential-store proof were merged in PR #21, closing issue #20.
- The account view uses an application-owned workflow and a provider-neutral account profile. The Microsoft provider uses OAuth authorization code with PKCE, Graph `/me`, and Linux Secret Service for refresh credentials. Synthetic provider and headless UI tests cover the account flow.
- Issue #45 tracks the follow-up that preserves account operation results when the view cancels while credential persistence is pending. Its changes are not yet part of this candidate.
- The Linux credential proof covers Fedora GNOME Keyring with the service unlocked. It uses synthetic credentials and does not access a real mailbox.
- No live Graph request or Entra sign-in has been performed. Source review and synthetic fixtures do not prove browser, callback, MFA, Conditional Access, consent revocation, refresh, or sign-out behavior against a tenant.

## Active risks

- No dedicated Microsoft 365 test tenant or public app registration is available. Keep issue #3 open until its live evidence is complete.
- The registered callback port, default-browser launch, and Entra consent behavior remain unverified against a tenant.
- Fedora GNOME Keyring results do not establish behavior across every Secret Service implementation.
- Windows and macOS build, runtime, credential storage, installation, and operating-system integration remain deferred.

## Next actions

1. Add the issue #45 cancellation changes to the local M1 candidate and verify them with synthetic tests.
2. Review the candidate for issues #5 and #45 before publishing it.
3. Keep issue #3 open until a dedicated tenant and public app registration are available for the opt-in live proof.
4. Continue M2 only after the M1 account and storage gates are reviewed.

## Update rule

Update this file when the milestone, an active risk, or a next action changes. Keep implementation plans and acceptance criteria in GitHub issues.
