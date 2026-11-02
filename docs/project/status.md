# Project status

Last updated: 2026-11-02

## Current milestone

M2 read-only mail is assembled locally on `candidate/m2-read-only-mail`, based on `origin/main` after PR #46 merged. It adds Microsoft Graph mail transport, encrypted offline sync and cached browsing, sanitized native message rendering, and durable recovery when a message delta cursor expires. The local Linux gate and source review passed on 2026-11-02. The candidate is not published. No live Entra or Graph request has been made.

PR #46 merged into `main` on 2026-11-02. It contains the M1 account connection and cancellation boundary. Keep issue #3 open until live Entra checks pass.

## Verified state

- The Linux GPUI Kit foundation and encrypted-cache/Secret Service proof are merged. Linux is the current development and validation target; Windows and macOS support are deferred under [ADR 0010](../decisions/0010-linux-first-development.md).
- The account view uses an application workflow and provider-neutral profile. The Microsoft provider uses authorization code with PKCE, Graph `/me`, and Linux Secret Service for refresh credentials. Cancellation is ordered against credential commits so the view reports the operation's actual outcome.
- The M2 candidate contains read-only folder and message synchronization, a fixed 90-day cutoff, encrypted transactions that commit each page and cursor together, fetch-on-open bodies, offline cached browsing, keyboard navigation, and native rendering of sanitized message content. Expired message delta cursors start a durable replacement scan while cached messages and bodies remain available. The candidate contains no calendar workflows or mail-write actions.
- Automated fixtures use synthetic data. The candidate includes Graph transport, persistence, sanitizer, and headless GPUI proof packages. The combined Linux gate passed locally on 2026-11-02; the RustSec audit found no vulnerable or yanked locked crates and reported the eight upstream unmaintained transitive crates already tracked in [dependency review](dependency-review.md).
- No live Entra sign-in or Graph mailbox request has been performed. The candidate does not claim tenant verification.

## Active risks

- No dedicated Microsoft 365 test tenant or public app registration is available. Keep issue #3 open and leave issue #8's tenant-only sync evidence unverified.
- Issue #9 remains open for real-window layout, operating-system clipboard behavior, and system-browser launch. Do not launch a GUI during this work.
- Fedora GNOME Keyring evidence does not establish behavior for every Secret Service implementation.
- Windows and macOS build, runtime, credential storage, installation, and operating-system integration remain deferred.

## Next actions

1. Keep issue #8 open until initial-sync completeness is verified against a Microsoft 365 test tenant.
2. Keep issue #9 open until real-window layout, clipboard behavior, and browser launch are verified in a safe Linux session.
3. Keep M2 local until the publication decision is complete.

## Update rule

Update this file when the milestone, an active risk, or a next action changes. Keep implementation plans and acceptance criteria in GitHub issues.
