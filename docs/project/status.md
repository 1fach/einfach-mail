# Project status

Last updated: 2026-11-06

## Current milestone

M4 mail and calendar actions remain in progress. PR #47 merged read-only mail into `main` at `409d299ffbe`; PR #50 merged the Linux calendar agenda and its reviewed stack into `main` on 2026-11-06 as merge commit `8c10dca99fd8cc0a38f76fb11aed5a174b0970f8`. All four required PR #50 checks passed on head `0ecfc019ff661282bc6bf5bfe5ce522bbb77c4e3`. PR #51 implements issue #49 calendar cursor recovery on `main` after PRs #47 and #50 merged. ADR 0036 records the reset flow, ADR 0043 records the per-account session boundary, and ADR 0044 records the event-action gate during replacement scans. Keep issue #49 open until a Microsoft 365 tenant confirms the cursor-reset response shape.

## Active risks

- No dedicated Microsoft 365 test tenant or public app registration is available. Keep live sign-in, initial mail sync, read-state, flag, send, reply, move, and calendar verification open under their existing issues.
- Manual Linux review of real-window layout, focus, scrolling, and screen-reader behavior remains open for issues #9 and #22. No GUI has been launched during this work.
- Windows and macOS support remain deferred.
- Fedora GNOME Keyring evidence does not establish behavior for every Secret Service implementation.

## Next actions

1. Keep issue #49 open until a Microsoft 365 tenant confirms the documented cursor-reset response shape.
2. Keep issue #28 open until a dedicated Microsoft 365 test tenant is available for delegated consent and live Graph send verification.
3. Continue the Linux real-window accessibility review tracked in issues #9 and #22.

## Update rule

Update this file when the milestone, an active risk, or a next action changes. Keep implementation plans and acceptance criteria in GitHub issues.
