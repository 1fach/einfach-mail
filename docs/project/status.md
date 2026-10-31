# Project status

Last updated: 2026-10-31

## Current milestone

M0, repository foundation. The workspace has a native GPUI Kit shell. Sign-in, mail, calendar, and synchronization are not implemented.

## Verified state

- `main` points to an empty root commit. `chore/einfach-mail-foundation` contains the project in one commit. Draft [PR #13](https://github.com/1fach/einfach-mail/pull/13) is unmerged.
- The foundation snapshot passed all eight checks in [CI run 41155947046](https://github.com/1fach/einfach-mail/actions/runs/41155947046) and [RustSec run 41155947067](https://github.com/1fach/einfach-mail/actions/runs/41155947067). The current PR page shows checks for its latest commit.
- Fedora 44 shell startup was verified on 2026-10-31. The window rendered the current `Einfach Mail` title. Issue #1 has an older screenshot with the previous title; no replacement screenshot was uploaded.
- `crates/domain` implements opaque account, folder, message, calendar, and event IDs. `crates/application` has no workflows yet. No provider, identity, persistence, or synchronization adapter exists.
- Dependency candidates and the eight unmaintained transitive-crate findings are recorded in the [dependency review](dependency-review.md). The strict audit found no vulnerable or yanked crates.

## Active risks

- GPUI Kit is pre-1.0. The native window has not been launched in Windows. [Issue #2](https://github.com/1fach/einfach-mail/issues/2) tracks that test. Windows CI checks builds and tests; it does not verify an interactive window. Keep M0 open until the launch test passes. M1 work can start after PR #13 lands and the RustSec audit passes on `main`, with issue #2 still open.
- The RustSec workflow passed on PR #13 but has not landed on `main`. Keep [issue #10](https://github.com/1fach/einfach-mail/issues/10) open until the workflow is merged and its default-branch run passes.
- GitHub's branch-protection and ruleset APIs returned 403 for this private repository plan on 2026-10-31. Use the manual check gate in [development setup](setup.md).
- Authentication, encrypted storage, Graph transport, and email rendering still need the proofs recorded in ADRs [0003](../decisions/0003-desktop-authentication.md), [0005](../decisions/0005-encrypted-local-cache.md), [0006](../decisions/0006-graph-http-transport.md), and [0007](../decisions/0007-message-body-rendering.md).

## Next actions

1. Review and land PR #13. After it reaches `main`, verify the RustSec workflow and close [issue #10](https://github.com/1fach/einfach-mail/issues/10) when it passes.
2. Start M1 proof work in [issues #3 and #4](https://github.com/1fach/einfach-mail/milestone/2) after the foundation lands and the main-branch audit passes. Keep [issue #2](https://github.com/1fach/einfach-mail/issues/2) open until a Windows environment is available; it can remain open while M1 starts.
3. Start account connection work in [issue #5](https://github.com/1fach/einfach-mail/issues/5) after the authentication proof in issue #3 passes.

## Update rule

Update this file when a milestone, blocker, or next action changes. Keep task-by-task history in issues and pull requests.
