# Project status

Last updated: 2026-11-06

## Current milestone

M4 mail and calendar actions remain in progress. PR #53 merged into `candidate/issue-22-calendar-sync` on 2026-11-06 as merge commit `379ba105a4a7836412d5c0f81a86085372739ae3`, completing the native plain-text send implementation and closing issue #27. The merged stack also includes read-state updates, follow-up flags, saved-account identity, the calendar agenda, and account-cache lifecycle coordination. All four required checks passed on PR head `b2d067d42594d5f67770c2925fa8d30c34ee0b98`.

## Active risks

- No dedicated Microsoft 365 test tenant or public app registration is available. Keep live sign-in, initial mail sync, read-state, flag, send, reply, move, and calendar verification open under their existing issues.
- Manual Linux review of real-window layout, focus, scrolling, and screen-reader behavior remains open for issues #9 and #22. No GUI has been launched during this work.
- Windows and macOS support remain deferred.
- Fedora GNOME Keyring evidence does not establish behavior for every Secret Service implementation.

## Next actions

1. Continue the M1, M2, and M3 verification work recorded in the roadmap and GitHub issues.
2. Keep issue #28 open until a dedicated Microsoft 365 test tenant is available for delegated consent and live Graph send verification.
3. Continue the Linux real-window accessibility review tracked in issues #9 and #22.

## Update rule

Update this file when the milestone, an active risk, or a next action changes. Keep implementation plans and acceptance criteria in GitHub issues.
