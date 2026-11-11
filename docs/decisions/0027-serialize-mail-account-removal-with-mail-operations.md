# ADR 0027: Serialize mail account removal with mail operations

- Status: Accepted
- Date: 2026-11-04
- Owners: Einfach Mail maintainers

## Context

An account session shares one encrypted cache client and one operation registry across its mail and calendar workflows. The registry tracks account, folder, and calendar-view work. Account removal reserves the account and waits for the cache adapter to check its durable pending-send marker before deleting files and the encryption key.

AccountView discarded the account session when a user returned from Mail or Calendar. A later sign-out then used the factory removal path, which opened a new cache client. A cache operation already queued on the session's worker could outlive its view task and overlap removal through the new client.

The in-memory registry also cannot coordinate separate desktop processes. [ADR 0028](0028-single-linux-desktop-process.md) defines the Linux process boundary.

## Decision

- Keep the account session in AccountView when the user returns to the account screen. Reuse that session when the user opens Mail or Calendar again.
- On sign-out, remove the cache through the retained account session. Use the factory path only when the user has not opened an account session.
- Keep the session after `OperationInProgress` or `SendAlreadyPending` so the user can reopen Mail or Calendar, resolve the conflict, and retry sign-out.
- Clear the session after cache removal succeeds. Clear it after other removal errors that may have closed the cache client, following [ADR 0029](0029-retry-cache-removal-after-sidecar-failure.md).
- Hold the shared account reservation through the pending-send check, cache-file removal, and key removal. Reject account removal while mail or calendar work owns that account.
- Keep the persistence marker check. It protects an unresolved send or reply after the application restarts.
- Keep the regression tests synthetic. Do not launch the GUI or send a live Graph request for this decision.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Retain and reuse the account session | Account removal uses the same reservation registry and cache worker as the views. | AccountView holds the cache client while the account remains connected. |
| Reopen a cache client for every sign-out | Keeps AccountView state smaller. | A second worker can overlap queued work from the session that the user just closed. |
| Add a persistent operation table for every mail and calendar action | Represents more active states across restarts. | Adds schema and recovery rules when a shared session, process lock, and existing send marker cover the current workflows. |

## Consequences

- Returning to Account does not discard the account session or its cache worker.
- A sign-out request fails with `OperationInProgress` while account, folder, or calendar-view work is active.
- A pending send or reply continues to block cache removal after its active reservation ends.
- Synthetic tests cover account sync, folder writes, body fetches, moves, sends, replies, marker checks, and the account-view retry path.
- This decision does not verify live Microsoft Graph behavior.

## Sources

- [GitHub issue #43: Serialize account removal with active mail operations](https://github.com/1fach/einfach-mail/issues/43)
- [ADR 0028: Allow one Linux desktop process per user data directory](0028-single-linux-desktop-process.md)
- [ADR 0029: Keep cache removal retryable after a sidecar failure](0029-retry-cache-removal-after-sidecar-failure.md)
- `apps/desktop/src/app/workspace.rs`: retained account session
- `apps/desktop/src/features/account/actions.rs`: sign-out ordering and retry
- `crates/application/src/mail_workflow.rs`: shared reservations and account cache removal
- `crates/persistence/src/store.rs`: durable pending-send check and cache deletion
