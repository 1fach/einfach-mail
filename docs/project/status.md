# Project status

Last updated: 2026-11-08

## Current milestone

M4 mail and calendar actions remain in progress.

PR #47 merged read-only mail into `main` at `409d299ffbe11dfe41c8f5a428b8e49b9ac97edd` on 2026-11-06. PR #50 merged the Linux calendar agenda and its reviewed stack into `main` at `8c10dca99fd8cc0a38f76fb11aed5a174b0970f8` on 2026-11-06. All four required PR #50 checks passed on head `0ecfc019ff661282bc6bf5bfe5ce522bbb77c4e3`.

PR #51 merged issue #49 calendar cursor recovery into `main` at `a31c775fc95e23d0b705efb6b8f7560b60c45e6f` on 2026-11-06. All four required checks passed on head `3b99b11a708a80bb02ef82553e2144165bd88ab7`. ADR 0036 records the reset flow, ADR 0043 records the per-account session boundary, and ADR 0044 records the event-action gate during replacement scans. Keep issue #49 open until a Microsoft 365 tenant confirms the cursor-reset response shape.

[PR #79](https://github.com/1fach/einfach-mail/pull/79) and [PR #80](https://github.com/1fach/einfach-mail/pull/80) added headless accessibility snapshots for the agenda and message list. [PR #81](https://github.com/1fach/einfach-mail/pull/81) merged into `main` at `54df53621d0bc4ac337cde88a68def0b2c5a9bd4` on 2026-11-07. It adds synthetic Graph coverage for recurring occurrences across a daylight-saving transition and completes issue #6's synthetic fixture criterion. Issue #6 remains open for live recurrence, time-zone, and all-day verification.

[PR #246](https://github.com/1fach/einfach-mail/pull/246) implements issue #245's durable per-view recovery for calendar event writes. Local Linux checks pass, including SQLCipher reopen and partial-view recovery tests. Live Graph behavior remains unverified under issues #40 and #42.

## Active risks

- No dedicated Microsoft 365 test tenant or public app registration is available. Keep live sign-in, initial mail sync, read-state, flag, send, reply, move, and calendar verification open under their existing issues.
- Issue #6 has synthetic coverage for expanded occurrences and UTC-midnight all-day inputs. Graph's docs do not say how a non-UTC all-day event appears in a UTC-default `calendarView` response. Live recurrence, time-zone, and all-day behavior remain unverified until a Microsoft 365 tenant is available.
- Manual Linux review remains open under issues #9 and #22. It covers real-window layout, focus, scrolling, screen-reader behavior, clipboard, and browser launch. No GUI has been launched during this work.
- Windows and macOS support remain deferred.
- The production-path cache-key lifecycle test passed on 2026-11-08 against a temporary GNOME Keyring service. This does not establish behavior for every Secret Service implementation.
- The 2026-11-08 local RustSec audit loaded 1,422 advisories and scanned 947 locked dependencies. It found eight transitive `INFO Unmaintained` crates, with seven in the active Linux graph and `rustls-pemfile` cross-target only. It found no vulnerable or yanked versions. GPUI Kit v0.9.0 remains the latest release. At [upstream commit `137070d`](https://github.com/longbridge/gpui-kit/commit/137070d7c4e5c7525fb3af5ac260b9c3fe700919), the manifest still pins `gpui-pre` 0.3.11, and the lockfile retains all eight advisory crate names. Recheck when GPUI Kit or its Linux renderer, HTTP, or zbus dependencies publish compatible updates. See the [dependency review](dependency-review.md) for paths and replacement checks.

## Next actions

1. Resume the tenant-backed checks in issues #3, #6, #8, #24, #26, #28, #30, #32, #34, #36, #38, #40, #42, and #49 when the required Microsoft 365 test tenant and public app registration are available.
2. Continue the Linux real-window accessibility review tracked in issues #9 and #22.
3. Recheck the transitive RustSec warnings when GPUI Kit or its Linux renderer, HTTP, or zbus dependencies publish compatible updates.

## Update rule

Update this file when the milestone, an active risk, or a next action changes. Keep implementation plans and acceptance criteria in GitHub issues.
