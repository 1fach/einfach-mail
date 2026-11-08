# ADR 0052: Persist calendar event write recovery

- Status: Accepted
- Date: 2026-11-08
- Owners: Einfach Mail maintainers

## Context

ADRs 0023 and 0026 require every previously synchronized calendar view to sync after an uncertain event update or deletion. SQLCipher now stores those recovery markers, but the application also mirrors the account-wide state in memory. The duplicate can drift from the persisted markers. The workflow factory still needs memory to track which views have synced in that factory, and the `CalendarCache` contract must state exactly when adapters clear persisted markers.

The cache already stores one synchronization record per account view. A durable recovery flag on that record can identify which views need reconciliation and can be updated in the same transactions as view sync and event cache changes. In-memory state still needs to distinguish a view synced in the current workflow factory from one that has not been opened and synced there.

## Decision

- Add a `write_sync_required` flag to each `calendar_view_sync_state` row. The migration sets it for existing rows with a completed sync so caches from earlier versions receive one reconciliation pass.
- While holding the account reservation, mark every previously synchronized view for recovery before calling the provider for an update or deletion. Set the markers before the provider's final event read so a crash at any later point leaves a durable gate.
- Check for account markers and active replacement scans before creating, updating, or deleting an event. Return `CalendarWriteSyncRequired` while any view still needs recovery.
- Keep application memory only for views that completed a sync in the current workflow factory. A new factory requires each view to sync before it can write, even when that view has a saved cursor. Do not mirror account-wide post-write recovery in memory.
- Clear one view's marker in the same transaction that commits its completed delta page and cursor. Partial pages, failed syncs, and interrupted replacement scans do not clear the marker.
- Clear all account markers in the same transaction that commits a confirmed update or deletion to every cached view. Preserve markers after an unknown result, a dropped request future, a mismatched confirmation, or a cache failure.
- Clear markers after a provider error only when the result is definitive and no stale-event reconciliation is needed. If marker cleanup fails, the persisted markers remain and continue blocking writes.
- Keep event creation's durable transaction-ID marker under ADR 0016. The account marker check also blocks creation while update or deletion recovery remains.

The `CalendarCache` port documents these transaction effects for each adapter. Persistence owns the marker records and atomic cache changes. Application memory owns only per-factory view readiness. ADR 0047 continues to define how those per-factory states are shared among sessions.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Keep recovery only in process memory | Avoids a schema change. | A restart loses the list of views that still need reconciliation. |
| Store one account generation and a generation per view | Makes recovery epochs explicit. | Adds generation state and transition rules across two persisted records. |
| Store a recovery flag on each calendar view row | Matches the per-view sync boundary and lets each completed sync clear only its own requirement. | Adds one SQLCipher column and an account-wide query before event writes. |

Use the per-view flag. It records the exact recovery requirement beside the cursor and view snapshot it protects.

## Consequences

- Calendar post-write recovery survives application restarts and cache-worker shutdowns.
- A cache upgrade marks previously synchronized views for one delta sync before another event write.
- The application has one in-memory rule for view readiness in the current factory. SQLCipher is the source of truth for account-wide post-write recovery.
- Confirmed cache updates clear recovery markers atomically with their event changes. A completed delta commit clears only the view it reconciles.
- Graph request and response contracts do not change. Live Exchange Online behavior remains unverified under issues #40 and #42.

## Sources

- [ADR 0023: Edit standalone calendar appointments](0023-edit-standalone-calendar-appointments.md)
- [ADR 0026: Delete standalone calendar appointments](0026-delete-standalone-calendar-appointments.md)
- [ADR 0047: Share write readiness across account sessions](0047-share-write-recovery-across-account-sessions.md)
- [Issue #245: Persist event write recovery across restarts](https://github.com/1fach/einfach-mail/issues/245)
- [Issue #247: Make persisted calendar write recovery authoritative](https://github.com/1fach/einfach-mail/issues/247)
