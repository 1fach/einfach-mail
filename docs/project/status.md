# Project status

Last updated: 2026-10-31

## Current milestone

M1 identity and local-data proofs. Linux is the sole development and validation target. The native GPUI Kit foundation and Fedora shell launch are complete for Linux. Windows and macOS work is deferred under [ADR 0010](../decisions/0010-linux-first-development.md). Account sign-in, mail, calendar, and synchronization are not implemented in the application.

## Verified state

- `main` contains the native foundation as one commit above the empty root. [PR #13](https://github.com/1fach/einfach-mail/pull/13) was rebase-merged on 2026-10-31. Fedora shell startup was verified in a Wayland session.
- The current branch narrows development CI and dependency checks to Linux. `origin/main` still has the earlier cross-platform CI until this change is merged.
- The test-only [storage proof](../../tools/storage-proof/) uses SQLCipher-backed SQLite and Linux Secret Service through `keyring` 4.2.0. Two ignored credential lifecycle tests passed in Fedora GNOME Keyring on 2026-10-31 with synthetic credentials. The updated missing-key check confirms that the encrypted database and backup remain byte-for-byte unchanged when the credential is missing. The unavailable-service path does not create a cache or modify existing encrypted files. Issue #20 remains open until these changes pass PR review.
- The earlier Windows Credential Manager proof in [PR #15](https://github.com/1fach/einfach-mail/pull/15) is historical, platform-specific evidence. It does not establish Windows support.
- `crates/application` has no workflows, and no production identity, provider, persistence, or synchronization adapter exists.

## Active risks

- The Linux credential proof covers Fedora GNOME Keyring with the service unlocked. A synthetic `NoStorageAccess` case covers the locked-store error path; behavior across other Secret Service implementations remains unverified.
- Portable backup recovery without the operating-system credential remains open in [ADR 0004](../decisions/0004-local-data-and-sync.md).
- Windows and macOS build, runtime, credential storage, installation, and operating-system integration are deferred. Linux CI does not establish support for those platforms.
- Authentication still needs the Linux system-browser and Microsoft 365 test-tenant proof in [ADR 0003](../decisions/0003-desktop-authentication.md) and issue #3.

## Next actions

1. Complete review and Linux CI for the issue #20 PR. Merge the PR and close the issue after both pass.
2. Run the opt-in Linux Entra proof with a dedicated test tenant and complete the required consent, refresh, and sign-out checks for issue #3.
3. Start account connection work in issue #5 after the Linux authentication and credential-storage gates pass.
4. Begin M2 read-only mail sync after M1 gates pass. Keep Windows and macOS implementation work in their later roadmap milestones.

## Update rule

Update this file when the milestone, an active risk, or a next action changes. Keep implementation plans and acceptance criteria in GitHub issues.
