# ADR 0019: Set follow-up flags on cached messages

- Status: Accepted
- Date: 2026-11-01
- Owners: Einfach Mail maintainers

## Context

The native mail view already lets a user set a cached message's read state. The next M4 action lets the user flag that message for follow-up or clear the flag.

Microsoft Graph v1.0 exposes a message's `flag` as a `followupFlag` with the statuses `notFlagged`, `flagged`, and `complete`. The message update endpoint accepts `flag` through `PATCH /me/messages/{id}` and lists delegated `Mail.ReadWrite` as its least-privileged permission. The app already requests that scope for the read-state action.

Message delta requests select an explicit list of properties. Graph stores those query options in each opaque `@odata.nextLink` and `@odata.deltaLink`. Existing cursors were created without `flag`, so they cannot track flag changes. A cache migration must start a new delta round while preserving the cached messages and bodies.

No dedicated Microsoft 365 test tenant is available. Local work uses synthetic responses. Issue #32 tracks live consent, delta, and update-response evidence.

Flag updates use the same message-PATCH response limit and unknown-outcome contract recorded in [ADR 0015](0015-idempotent-message-read-state-update.md).

## Decision

- Add **Flag** to the native view for an opened cached message. Set `flagged` when the cached status is `notFlagged` or unknown. Set `notFlagged` when the status is `flagged`. Set `flagged` and label the action **Reopen flag** when the status is `complete`. This first action does not mark a flag complete.
- Model the three Graph statuses as `FollowUpFlagStatus`. Store `Option<FollowUpFlagStatus>` in `MessageMetadata`; `None` means the cache has no known server status. Keep Graph parsing in the Microsoft provider. Reject unknown status strings at that boundary.
- Add `flag` to the selected message metadata for list and delta requests. Preserve sparse-update semantics: an omitted property leaves the cached status unchanged, and an explicit `null` clears the local status to unknown.
- Add a nullable, constrained `flag_status` column to `mail_messages`. Bump the message query version and clear each folder's `delta_link`, `next_link`, and last-sync time during the migration. Preserve the fixed account cutoff, cached messages, opened bodies, and folder data. Existing flag values stay `NULL` until the new initial delta round supplies them.
- Set a requested `FollowUpFlagStatus` for a message already cached in the caller's account and folder. Reserve the folder against a concurrent delta sync and commit the shared account-folder recovery marker before dispatching the PATCH. If the marker cannot be committed, do not send the request. Send one `PATCH /me/messages/{id}` containing only `flag.flagStatus`. Do not automatically retry the PATCH. Accept only `200 OK`, and require the response to contain the requested status before reporting confirmation.
- Never retry a flag PATCH automatically. Treat a timeout, connection loss, server error, unexpected success status, unreadable or oversized `200 OK` response, or response without the requested flag status as an unknown outcome. Leave the cache unchanged and ask the user to sync the folder before retrying the same desired status.
- Persist the confirmed status only after Graph confirms it. If the cache write fails, report that Graph confirmed the change and tell the user to sync the folder. Keep the visible state aligned with the confirmed response.
- Keep the recovery marker defined in [ADR 0015](0015-idempotent-message-read-state-update.md) through the PATCH and cache update. On a definite rejection, remove it when SQLCipher confirms cleanup. On confirmation, remove it only after SQLCipher commits the requested status. Unknown outcomes and confirmed cache failures leave the marker in place. This also covers an app exit while PATCH is in flight; after restart, a complete delta round must commit before another read-state or flag write.
- Use the existing delegated `Mail.ReadWrite` permission review, account matching, Graph transport, Serde boundary, SQLCipher cache, and headless GPUI test setup. Add no new crate; a provider-specific Graph request does not need a general-purpose library.
- Cover Graph parsing and request construction, delta updates, migration and restart, cache failures, account boundaries, and native UI feedback with synthetic data. Keep live verification in issue #32. Do not make a live request or launch the GUI for local implementation.

## Alternatives

| Option | Benefit | Cost or risk |
| --- | --- | --- |
| Add `flag` to delta selection and reset existing cursors during migration | The cache learns flag changes through its existing sync path and can display them offline. | Each folder must complete a new initial delta round. Existing messages and bodies remain available while that round runs. |
| Fetch `flag` only when the user opens or changes a message | Avoids a cursor reset and a metadata re-sync. | Adds a network request to an offline message view and creates separate synchronization behavior for one field. |
| Store only the state requested by the local action | Keeps the schema small. | The app cannot show flags set in Outlook, and a stale local value can make the next action unclear. |

## Consequences

- Existing caches keep their mail data and cutoff. Their message sync checkpoints reset once so Graph can track the expanded property selection.
- A cached message can have an unknown follow-up status until a folder finishes its new initial delta round. The action safely offers **Flag** for that state.
- Flag updates are explicit desired-state writes. An unknown outcome does not trigger an automatic retry. The SQLCipher recovery marker is committed before PATCH dispatch, survives cache reopen and process exit, and blocks read-state and flag writes to that folder until a complete delta round commits.
- `Mail.ReadWrite` permits creating, reading, updating, and deleting mail. The account view must continue to disclose that full permission even though this action only changes flags.
- Synthetic tests establish request construction and local state transitions. Issue #32 remains open until a dedicated tenant confirms real delta and PATCH behavior.

## Sources

- [Microsoft Graph `followupFlag` v1.0](https://learn.microsoft.com/en-us/graph/api/resources/followupflag?view=graph-rest-1.0)
- [Microsoft Graph update message v1.0](https://learn.microsoft.com/en-us/graph/api/message-update?view=graph-rest-1.0)
- [Microsoft Graph message delta v1.0](https://learn.microsoft.com/en-us/graph/api/message-delta?view=graph-rest-1.0)
- [Microsoft Graph delta query state tokens](https://learn.microsoft.com/en-us/graph/delta-query-overview#state-tokens)
- [Microsoft Graph permissions reference](https://learn.microsoft.com/en-us/graph/permissions-reference?view=graph-rest-1.0#mailreadwrite)
- [ADR 0011: Initial mail synchronization](0011-initial-mail-sync-scope.md)
- [ADR 0015: Update message read state](0015-idempotent-message-read-state-update.md)
- [Issue #31: Flag cached messages](https://github.com/1fach/einfach-mail/issues/31)
- [Issue #32: Verify follow-up flags in a test tenant](https://github.com/1fach/einfach-mail/issues/32)
