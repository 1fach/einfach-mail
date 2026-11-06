# Project status

Last updated: 2026-11-06

## Current milestone

M4 mail and calendar actions remain in progress. PR #52's retryable account-cache removal changes are merged into `candidate/issue-22-calendar-sync` as merge commit `ffe4755ce4ce1b42529dbd37a56254e141f855c9`; all four required GitHub checks passed before merge. PR #54's reviewed read-state implementation is merged into PR #53 at `9522d7be4690d3c14d89719511ca067b95108e35`; the same stack includes follow-up flags and saved-account identity.

PR #53 implements issue #27's single plain-text send. Its review worktree has integrated the latest `candidate/issue-22-calendar-sync` head `37a1c802a62ec781c8a7e5368ed0aea4f27efa34`, which carries the calendar agenda and account-cache lock changes. Source review found that a failed Linux `flock` upgrade could lose the cache's shared lock. The integrated branch now coordinates opens and removals with a lifecycle gate and restores shared ownership after failed removal. The source review and four exact-head GitHub checks still need to pass before merge. Issue #27 and PR #53 remain open; keep issue #28 open for tenant consent and Graph verification.

## Active risks

- No dedicated Microsoft 365 test tenant or public app registration is available. Keep live sign-in, initial mail sync, read-state, flag, send, reply, move, and calendar verification open under their existing issues.
- Manual Linux review of real-window layout, focus, scrolling, and screen-reader behavior remains open for issues #9 and #22. No GUI has been launched during this work.
- Windows and macOS support remain deferred.
- Fedora GNOME Keyring evidence does not establish behavior for every Secret Service implementation.

## Next actions

1. Finish the source and security review of the integrated PR #53 head while preserving root WIP.
2. Push the reviewed head once, wait for all four required exact-head checks, then post the review and merge PR #53 into `candidate/issue-22-calendar-sync` if it is clean.
3. After merge, update issue #27 and the project records; keep issue #28 open for tenant-only evidence.
4. Continue the independent M1, M2, and M3 verification work recorded in the roadmap and GitHub issues.

## Update rule

Update this file when the milestone, an active risk, or a next action changes. Keep implementation plans and acceptance criteria in GitHub issues.
