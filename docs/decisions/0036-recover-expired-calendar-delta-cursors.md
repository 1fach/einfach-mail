# ADR 0036: Recover expired calendar delta cursors

- Status: Accepted for Linux read-only calendar synchronization; live Graph verification remains open
- Date: 2026-11-04
- Owners: Einfach Mail maintainers

## Context

Calendar view synchronization stores a completed delta cursor and an interrupted page checkpoint for each account and exact UTC view range. When Graph expires a completed cursor, retrying it leaves the cached week stale. Clearing the cursor and applying a new scan directly to active rows can expose a mixed snapshot or leave that mixture after an interruption.

Microsoft's general delta guidance describes a reset response of `410 Gone` with a `Location` URL containing an empty `$deltatoken`. It lists `event` among Outlook delta resources and says an expired token can produce a 4XX error such as `syncStateNotFound`. The event-delta page does not specify which response Exchange Online returns. Keep issue #49 open for tenant confirmation. See [Microsoft Graph research](../research/microsoft-graph.md#mail-and-calendar-synchronization). Issue #6 tracks tenant verification for recurrence, time zones, and all-day events.

Issue #49 is part of M3, which is read-only. Calendar provider writes are outside this decision. [ADR 0044](0044-block-calendar-actions-during-replacement-sync.md) defines how the existing M4 event actions interact with interrupted replacement scans.

## Decision

The provider returns a typed reset only for a continuation request. It recognizes `410 Gone` only when Graph supplies a `Location` header. For other 4XX responses, it recognizes only the exact Graph error code `syncStateNotFound`. It reads that error through the existing bounded response reader and never exposes the raw error body. Authentication, throttling, network, server, and unrelated protocol errors retain their existing handling.

When Graph supplies a `Location`, the adapter accepts it only on the configured Graph authority, for `/me/calendarView/delta`, with the exact original `startDateTime` and `endDateTime` values and one empty `$deltatoken`. It rejects extra, duplicate, malformed, cross-origin, and range-mismatched query data. A `syncStateNotFound` response may omit `Location`; in that case the replacement scan starts with the fixed-range initial request.

Limit every calendar delta URL to 16 KiB before parsing or persistence. The normal Graph response-body limit and per-round byte limit continue to bound links returned in page bodies; the explicit URL limit also bounds a reset `Location` header.

The encrypted cache stores an active replacement generation and its optional next link in SQLCipher. It stages events for the same account and exact range. The active event snapshot, completed cursor, and last successful sync time remain unchanged. Each page and checkpoint commit together. A later attempt resumes from the saved reset or page link. Without a link, it repeats the fixed-range initial request.

Staging retains out-of-view occurrences for later sparse updates. It records removals so a sparse update cannot restore an event removed in the replacement generation. If a later update needs fields for an event removed in that generation, the cache rejects the page and keeps its prior checkpoint.

The final page applies its changes to the staging snapshot, atomically swaps that snapshot into the active event table, installs the new delta link and completion time, removes staging rows, and clears the active marker. Until that transaction succeeds, readers continue to see the previous complete snapshot.

Allow at most one reset response during a synchronization attempt. Persist a subsequent reset checkpoint before returning a reset-limit error so a later attempt can resume it. Keep page, response-byte, cursor-loop, and cancellation bounds in force across resets.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Stage a replacement snapshot and swap it on the final page | Offline reads keep a coherent complete snapshot through interruptions; reconciliation and cursor replacement are atomic. | Requires a SQLCipher staging table and a schema migration. |
| Update active rows as replacement pages arrive and defer deletions | Uses less temporary storage and resembles message-cache rebuilding. | Readers see a mixture of old and replacement values, and a failed scan can leave that mixture visible. |
| Clear the active snapshot and cursor before restarting | Needs little state and minimal migration work. | A failed scan removes the last usable offline calendar snapshot. |

## Consequences

- The application provider contract distinguishes a page from a reset, and the cache contract starts a resumable replacement generation.
- SQLCipher owns the staging snapshot. Staging rows are scoped to the exact calendar view and never appear in event-list queries before the final swap.
- No new dependency is needed. The existing `url`, `serde`, and bounded `reqwest` transport cover URL validation and Graph error parsing; replacement behavior remains an application rule.
- Synthetic Graph, application, and SQLCipher tests verify reset classification, link validation, snapshot stability, restart, and final reconciliation, including sparse pages and removals. They do not verify a live tenant response.
- This decision remains read-only. [ADR 0044](0044-block-calendar-actions-during-replacement-sync.md) defines the M4 gate for event actions during an active replacement scan.

## Sources

- [Microsoft Graph delta query overview](https://learn.microsoft.com/graph/delta-query-overview#synchronization-reset)
- [Microsoft Graph event delta v1.0](https://learn.microsoft.com/en-us/graph/api/event-delta?view=graph-rest-1.0)
- [Microsoft Graph calendar view delta details](https://learn.microsoft.com/graph/delta-query-events)
- [ADR 0014: Linux read-only calendar workflow](0014-linux-read-only-calendar-workflow.md)
- [ADR 0032: Recover expired message delta cursors](0032-recover-expired-message-delta-cursors.md)
