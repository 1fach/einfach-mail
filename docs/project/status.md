# Project status

Last updated: 2026-10-31

## Current milestone

M0 closeout and M1 identity and storage proofs. The one-commit native GPUI Kit foundation is on `main`. M0 remains open for the interactive Windows shell launch in issue #2. M1 proof work can proceed because the foundation is merged and the main-branch RustSec audit passed. Sign-in, mail, calendar, and synchronization are not implemented.

## Verified state

- `main` contains the foundation as one commit, `eebd273`, above the empty root commit. [PR #13](https://github.com/1fach/einfach-mail/pull/13) was rebase-merged on 2026-10-31.
- All eight checks passed on the reviewed foundation tree in [CI run 41162044746](https://github.com/1fach/einfach-mail/actions/runs/41162044746) and [RustSec run 41162044740](https://github.com/1fach/einfach-mail/actions/runs/41162044740). The default-branch audit passed in [RustSec run 41163440562](https://github.com/1fach/einfach-mail/actions/runs/41163440562). All jobs passed in the post-merge [main CI run 41163440578](https://github.com/1fach/einfach-mail/actions/runs/41163440578), including Windows full Rust checks.
- [Issue #10](https://github.com/1fach/einfach-mail/issues/10) is closed after the default-branch audit passed.
- Fedora 44 shell startup was verified on 2026-10-31. The window rendered the current `Einfach Mail` title. Issue #1 has an older screenshot with the previous title; no replacement screenshot was uploaded.
- `crates/domain` implements opaque account, folder, message, calendar, and event IDs. `crates/application` has no workflows yet. No provider, identity, persistence, or synchronization adapter exists.
- Dependency candidates and the eight unmaintained transitive-crate findings are recorded in the [dependency review](dependency-review.md). The strict audit found no vulnerable or yanked crates.
- The test-only [storage proof package](../../tools/storage-proof/) locks the candidate SQLCipher, migration, async-wrapper, and Windows keyring dependencies as dev-dependencies. Linux and Windows x64 CI passed in [PR #15](https://github.com/1fach/einfach-mail/pull/15), including vendored OpenSSL, migrations, encrypted backup, wrong-key rejection, Windows Credential Manager, missing-key behavior, and account removal. ADR 0005 is accepted from this proof. No production persistence dependency or cache API exists yet.
- The test-only Graph transport proof passes 13 synthetic contract tests on Linux, Windows, and macOS in [CI run 41176065254](https://github.com/1fach/einfach-mail/actions/runs/41176065254). Rust 1.95 checks pass on all three platforms, and [RustSec run 41176065229](https://github.com/1fach/einfach-mail/actions/runs/41176065229) found no vulnerable or yanked crates. Any test-tenant evidence needed for behavior that fixtures cannot establish remains outstanding; ADR 0006 is still proposed.

## Active risks

- GPUI Kit is pre-1.0. The native window has not been launched in Windows. [Issue #2](https://github.com/1fach/einfach-mail/issues/2) tracks that test. Windows CI checks builds and tests; it does not verify an interactive window. Keep M0 open until the launch test passes. The foundation and main-branch audit gates for M1 are satisfied; issue #2 can stay open while M1 proceeds.
- GitHub's branch-protection and ruleset APIs returned 403 for this private repository plan on 2026-10-31. Use the manual check gate in [development setup](setup.md).
- Authentication, production Graph integration, and email rendering still need the proofs recorded in ADRs [0003](../decisions/0003-desktop-authentication.md), [0006](../decisions/0006-graph-http-transport.md), and [0007](../decisions/0007-message-body-rendering.md). The accepted storage decision does not yet have production persistence code.
- Cache backups stay encrypted and use the same credential-store key; portable recovery without that key remains an open decision.

## Next actions

1. Finish [issue #7](https://github.com/1fach/einfach-mail/issues/7) by obtaining test-tenant evidence for any Graph behavior that fixtures cannot establish. Keep ADR 0006 proposed until its evidence is complete.
2. Use the completed authentication library comparison in the [dependency review](dependency-review.md) to guide [issue #3](https://github.com/1fach/einfach-mail/issues/3). Windows and Microsoft test-tenant evidence remain required before accepting ADR 0003.
3. Start account connection work in [issue #5](https://github.com/1fach/einfach-mail/issues/5) after the authentication proof in issue #3 passes. Keep [issue #2](https://github.com/1fach/einfach-mail/issues/2) open until a Windows environment is available; it can remain open while M1 proceeds.

## Update rule

Update this file when a milestone, blocker, or next action changes. Keep task-by-task history in issues and pull requests.
