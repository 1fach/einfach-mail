# ADR 0044: Block calendar actions during replacement sync

- Status: Accepted
- Date: 2026-11-06
- Owners: Einfach Mail maintainers

## Context

ADR 0036 keeps the last complete event snapshot visible while a replacement scan runs. M4 also supports event creation, editing, and deletion. If an event action changes the cache while a replacement is active, the final replacement commit can replace that change with its older staged data.

The replacement marker is stored per calendar view in SQLCipher. Workflow memory alone cannot tell whether another view for the same account is being replaced. Checking the marker alone also leaves a race: an action could pass its check just before another view starts syncing and persists the marker.

## Decision

Event operations reserve the account while view synchronization reserves its view. The shared reservation rejects an event operation if any view sync is active and rejects a new view sync if an event operation owns the account. This closes the race where an action could pass its persisted-marker check just before another view records a replacement.

The cache reports whether any account view has an active replacement scan in SQLCipher. If a replacement is interrupted, `load_cached_view` returns `CalendarWriteReadiness::AccountSyncRequired`, and `editable_event`, `create_event`, `update_event`, and `delete_event` check that persisted state before calling the provider. Completing the replacement clears this gate. Other write-recovery checks still apply.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Check the persisted account marker and serialize event operations with view syncs | Prevents event actions from racing active syncs and keeps writes blocked after an interrupted replacement, including overlapping views. | Adds one bounded cache query and blocks actions while any account view is syncing. |
| Check only the selected view | Adds a smaller query. | An action from another overlapping view could change the active cache while the replacement snapshot is staged. |
| Clear the active snapshot when a cursor expires | Removes the stale snapshot before another action can use it. | Discards the last complete offline view and changes ADR 0036's recovery behavior. |

## Consequences

- `CalendarCache` exposes whether any view for an account has an active replacement marker.
- Shared account reservations prevent event actions from racing an in-flight view sync; the persisted replacement marker keeps the account gated after an interrupted sync.
- The desktop can keep the prior snapshot readable while it disables event actions for that account.
- Synthetic application and SQLCipher tests verify that actions stop before provider calls and resume after replacement completes, including the concurrent sync and create case.
- This decision adds no dependency. Live Exchange Online reset behavior remains unverified without a test tenant.

## Sources

- [ADR 0036: Recover expired calendar delta cursors](0036-recover-expired-calendar-delta-cursors.md)
- [ADR 0023: Edit standalone calendar appointments](0023-edit-standalone-calendar-appointments.md)
- [ADR 0026: Delete standalone calendar appointments](0026-delete-standalone-calendar-appointments.md)
