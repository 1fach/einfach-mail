# ADR 0043: Block calendar actions during replacement sync

- Status: Accepted
- Date: 2026-11-06
- Owners: Einfach Mail maintainers

## Context

ADR 0036 keeps the last complete event snapshot visible while a replacement scan runs. M4 also supports event creation, editing, and deletion. If an event action changes the cache while a replacement is active, the final replacement commit can replace that change with its older staged data.

The replacement marker is stored per calendar view in SQLCipher. Workflow memory alone cannot tell whether another view for the same account is being replaced.

## Decision

The application blocks event actions while any view for the account has an active replacement scan. The cache reports that account state from its persisted view markers.

`load_cached_view` returns `CalendarWriteReadiness::AccountSyncRequired` while a replacement is active. `editable_event`, `create_event`, `update_event`, and `delete_event` check the persisted account state before calling the provider. Clearing the replacement marker removes this gate. Other write-recovery checks still apply.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Check every persisted view for the account | Prevents event actions from racing any active replacement scan, including overlapping views. | Adds one bounded cache query before each event action. |
| Check only the selected view | Adds a smaller query. | An action from another overlapping view could change the active cache while the replacement snapshot is staged. |
| Clear the active snapshot when a cursor expires | Removes the stale snapshot before another action can use it. | Discards the last complete offline view and changes ADR 0036's recovery behavior. |

## Consequences

- `CalendarCache` exposes whether any view for an account has an active replacement marker.
- The desktop can keep the prior snapshot readable while it disables event actions for that account.
- Synthetic application and SQLCipher tests verify that actions stop before provider calls and resume after replacement completes.
- This decision adds no dependency. Live Exchange Online reset behavior remains unverified without a test tenant.

## Sources

- [ADR 0036: Recover expired calendar delta cursors](0036-recover-expired-calendar-delta-cursors.md)
- [ADR 0023: Edit standalone calendar appointments](0023-edit-standalone-calendar-appointments.md)
- [ADR 0026: Delete standalone calendar appointments](0026-delete-standalone-calendar-appointments.md)
