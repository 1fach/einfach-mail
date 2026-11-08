# ADR 0051: Defer a calendar identifier until calendar selection exists

- Status: Accepted
- Date: 2026-11-08
- Owners: Project maintainer

## Context

ADR 0009 defines distinct opaque identifiers for accounts, folders, messages, calendars, and events. The current calendar workflow only displays the signed-in user's primary calendar. Its view reference is already scoped to an account and time range, and no application workflow consumes a provider calendar identifier. `CalendarId` therefore has no caller or invariant to enforce today.

## Decision

Do not expose a `CalendarId` domain type until a supported workflow needs to identify or select a calendar. Keep calendar event identifiers distinct and account-scoped as defined by ADR 0009. This decision supersedes only the calendar-identifier portion of ADR 0009.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Add `CalendarId` when a workflow needs it | Keeps the domain API limited to used concepts and ties the identifier to a concrete account-scoped operation. | Calendar selection will need a domain and application API change at that time. |
| Keep an unused `CalendarId` now | Matches the broader identifier rule in ADR 0009. | Adds a public type with no caller or behavior and implies calendar identity is already part of the current workflow. |

## Consequences

- The domain exports only identifier types used by current account, mail, and calendar-event workflows.
- Adding calendar selection or another workflow that addresses a calendar requires revisiting this decision and introducing an account-scoped calendar reference.
- The primary-calendar workflow remains unchanged.

## Sources

- [ADR 0009: Use account-scoped opaque domain identifiers](0009-account-scoped-domain-identifiers.md)
- [ADR 0014: Implement a cached Linux read-only primary calendar](0014-linux-read-only-calendar-workflow.md)
- [Calendar view reference](../../crates/domain/src/calendar.rs)
