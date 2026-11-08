# ADR 0045: Persist calendar event write recovery

- Status: Accepted
- Date: 2026-11-08
- Owners: Einfach Mail maintainers

## Context

ADRs 0023 and 0026 require every previously synchronized view for an account to sync after an uncertain event update or deletion. `AccountWorkflowRecovery` keeps that gate in process memory. A new process starts with an empty recovery map and requires the selected view to sync, but it does not know which other completed views remain in SQLCipher. Syncing one view can therefore permit another event write while a different cached view remains unreconciled.

This decision makes that recovery state durable. It supersedes the no-migration implementation consequences in ADRs 0023 and 0026 for write-recovery metadata; event payloads and cached event records do not change.

## Decision

- Add a `write_sync_required` flag to each `calendar_view_sync_state` row. The migration sets it for existing rows with a completed sync so caches from earlier versions receive one safe reconciliation pass.
- While holding the account reservation, set the flag for every completed account view before calling the provider for an event update or deletion. Set it before the provider's final GET so a crash at any later point leaves a durable gate.
- Check for any flagged view before creating, updating, or deleting an event. Return `CalendarWriteSyncRequired` while any account view still needs sync.
- Clear one view's flag in the same transaction that commits its completed delta page and cursor. Partial pages, failed syncs, and interrupted replacement scans leave the flag set.
- Clear all flags in the same transaction that commits a confirmed event update or deletion to every cached view. Preserve the flags after an unknown result, a dropped request future, or a cache failure.
- Clear flags after a provider error only when the result is definitive and no stale-event reconciliation is needed. Keep them after version conflicts and unknown outcomes. If cleanup fails, keep the in-process gate and leave any persisted flags in place.
- Keep event creation's durable transaction-ID marker under ADR 0016. The account-level recovery check blocks event creation while update or deletion recovery remains.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Keep recovery only in process memory | Avoids a schema change. | A restart loses the list of views that still need reconciliation. |
| Store one account generation and a generation per view | Makes recovery epochs explicit. | Adds generation state and transition rules across two persisted records. |
| Store a recovery flag on each calendar view row | Matches the existing per-view sync boundary and lets each completed sync clear only its own requirement. | Adds one SQLCipher column and an account-wide query before event writes. |

Use the per-view flag. It records the exact recovery requirement beside the cursor and view snapshot it protects.

## Consequences

- Calendar view recovery survives application restarts and cache-worker shutdowns.
- A cache upgrade marks previously synchronized views for one delta sync before another event write.
- The persistence adapter owns the durable flags. The application owns when to set and clear them. Graph request and response contracts do not change.
- The change adds no dependency or event data. It adds one cache-state column without changing cached event records or Graph contracts. Live Exchange Online behavior remains unverified under issues #40 and #42.

## Sources

- [ADR 0023: Edit standalone calendar appointments](0023-edit-standalone-calendar-appointments.md)
- [ADR 0026: Delete standalone calendar appointments](0026-delete-standalone-calendar-appointments.md)
- [Issue #245: Persist event write recovery across restarts](https://github.com/1fach/einfach-mail/issues/245)
