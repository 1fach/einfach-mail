# ADR 0014: Implement a cached Linux read-only primary calendar

- Status: Accepted
- Date: 2026-11-04
- Owners: Project maintainer

## Context

The application already owns account state, provider-neutral mail workflows, the Graph adapter, and encrypted per-account SQLCipher storage. The calendar must use those same boundaries and remain available offline. Microsoft Graph v1.0 supports delta synchronization for a fixed `calendarView` range. Its opaque cursor is scoped to that range, and the response can include recurring occurrences, exceptions, removals, and sparse updates.

The first calendar view is a Linux weekly agenda for the signed-in user's primary calendar. The user has no dedicated Microsoft 365 test tenant, so local implementation can prove parsing, synchronization, persistence, and presentation only. It cannot establish live recurrence or all-day behavior.

## Decision

- Implement read-only browsing of the signed-in user's primary calendar with Graph v1.0 `calendarView/delta`. Do not add calendar selection, event writes, reminders, or client-side recurrence expansion.
- Compute each visible week in the Linux system time zone. Send Graph UTC start and end instants and retain the local date bounds for all-day filtering. Persist each cursor under the account and exact range; a different week is a separate scope.
- Keep occurrences, sparse patches, removals, ranges, and opaque cursors provider-independent in the domain and application. Store timed ranges as UTC instants, all-day ranges as half-open dates, and source time-zone IDs as opaque strings.
- Parse and validate complete event records at the Graph boundary. Graph calendar-view delta does not support `$select`. Apply sparse updates only to a cached occurrence, reject an incomplete new occurrence, and remove records Graph reports as removed from the range.
- Store event rows and range cursors in the existing encrypted account cache. Commit each page and its next or completed cursor atomically. Retain cached events when refresh fails and remove them through account-cache removal.
- Request delegated `Calendars.Read` alongside the accepted delegated identity and mail scopes. Explain each permission before opening the system browser. Do not request write or application permissions.
- Render the weekly agenda in native GPUI. Keep provider and persistence calls out of views. Use Jiff only in Linux presentation code, format timed events in the detected system zone, label the UTC fallback, and never convert all-day dates through a time zone.
- Use synthetic Graph, workflow, persistence, and headless UI tests. Keep issue #6 open for live recurrence, time-zone, and all-day verification. Keep manual screen-reader and pixel review open until it can be performed safely in a Linux desktop session.

The per-account session that joins mail and calendar workflows is defined in [ADR 0043](0043-per-account-mail-and-calendar-session.md).

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Fixed-range delta sync in the existing encrypted cache | Supports offline weekly browsing and ties each cursor to the query that produced it. | Adds calendar tables and a calendar workflow; Graph semantics remain unverified against a tenant. |
| Fetch the visible week without caching | Less persistence and cursor code. | Does not meet the offline outcome and repeats full reads on each open. |
| Use one rolling multi-month cursor | Could support a month view later. | Fetches speculative data and changes the fixed-range cursor invariant. |
| Add a third-party Graph SDK | Could provide generated request models. | The reviewed Rust clients do not meet the existing transport, authority, error-header, and logging constraints recorded in ADR 0006. |

## Consequences

- Calendar permission is part of the first authorization request and must be explained accurately.
- Event IDs and opaque delta links remain scoped to the account and exact range; cursors are never shown or logged.
- Each page and its cursor commit in one transaction, so a retry converges on the same cached state.
- Calendar failures preserve cached events and do not prevent mail browsing.
- Synthetic Linux tests do not prove live Graph recurrence, time-zone, or all-day semantics, and the implementation does not establish Windows or macOS support.

## Sources

- [Microsoft Graph event delta](https://learn.microsoft.com/en-us/graph/api/event-delta?view=graph-rest-1.0)
- [Microsoft Graph delta query for events](https://learn.microsoft.com/en-us/graph/delta-query-events)
- [Microsoft Graph permissions reference](https://learn.microsoft.com/graph/permissions-reference?view=graph-rest-1.0)
- [ADR 0006: Use a private Graph HTTP transport](0006-graph-http-transport.md)
- [ADR 0005: Use SQLCipher-backed SQLite for the local cache](0005-encrypted-local-cache.md)
- [ADR 0043: Use one per-account mail and calendar session](0043-per-account-mail-and-calendar-session.md)
