# ADR 0020: Move cached messages between visible folders

- Status: Accepted
- Date: 2026-11-01
- Owners: Einfach Mail maintainers

## Context

The product brief includes organizing mail, and the native message view already supports read-state changes, follow-up flags, plain-text replies, and cached offline reading. Issue #33 adds a way to move one opened cached message into another visible folder in the same account.

Microsoft Graph v1.0's message move action accepts `destinationId` and returns `201 Created` with the moved message. It creates a copy in the destination folder and removes the original. The app already requests immutable Outlook message IDs on Graph operations. Microsoft documents that an immutable message ID stays the same when the item moves between folders in the same mailbox. Delegated `Mail.ReadWrite` is the least-privileged permission for the move action and is already requested for message updates.

The local cache keys messages by account, folder, and immutable message ID; it stores opened bodies in the same SQLCipher-protected row. Delta synchronization can temporarily cache the same immutable message ID in two folders because each folder advances its cursor independently. `synchronize_account` currently replaces the folder tree before taking per-folder reservations. A move needs an atomic source/destination reservation, and account synchronization must protect the folder-tree replacement and all folder pages from concurrent moves.

No dedicated Microsoft 365 test tenant is available. Issue #34 tracks tenant-only move, immutable-ID, and two-folder delta evidence. Local implementation uses synthetic responses and must not claim live behavior.

## Decision

- Add a **Move to…** native dropdown menu to the opened cached-message view. List visible folders in the active account, exclude the current folder, and display parent names to distinguish duplicate folder names. Use GPUI Kit's `DropdownMenu` and `PopupMenuItem` instead of custom popup behavior.
- Add a provider-neutral `move_message` workflow that requires a cached source message and a distinct destination folder from the same account's visible cached folder tree. Reserve the source and destination together before any provider request. Do not queue an offline move.
- Make account synchronization acquire an account-wide reservation before it lists or replaces the folder tree. Hold that reservation until all folder delta pages finish. This prevents tree replacement or either delta feed from changing the move's local source or destination during the Graph request and cache transaction.
- Send exactly one Graph v1.0 `POST /me/messages/{id}/move` with only `destinationId` and `Prefer: IdType="ImmutableId"`. Reuse delegated `Mail.ReadWrite` and the accepted Reqwest/Serde provider boundary. Do not automatically retry the POST.
- Require `201 Created`, the same immutable message ID, and a `parentFolderId` matching the requested destination before changing the cache. Treat a lost response, server error, unexpected success status, oversized/truncated response, or malformed/mismatched confirmation as an unknown outcome. Leave the cached source row in place and require account synchronization before another attempt.
- After exact Graph confirmation, move the cache row to the destination in one SQL transaction. Preserve the message ID, metadata, and encrypted body. If the destination already contains the same immutable ID, keep its existing metadata, fill its missing nullable metadata from the source, keep its body when present, copy the source body only when the destination has none, and delete the source row in the same transaction.
- Return separate outcomes for a move that was confirmed and cached and a move Graph confirmed while the cache needs synchronization. On a confirmed result, remove the source item from the current native view. Offer to open the destination after a successful cache move; show a sync action when the cache could not be updated. Keep an uncertain source item visible with an alert that asks the user to sync before retrying.
- Require a successful account sync and cache snapshot in the current session before enabling message moves. This makes startup synchronization the recovery path after a process restart without storing a durable pending-move marker. Keep moves disabled when startup sync fails.
- Add no new crate or schema migration. Use the existing Graph transport and JSON parser, SQLCipher schema, application cache boundary, and GPUI Kit menu. Keep the tenant-only proof in issue #34.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Confirm the Graph move, then atomically rehome the cached row | The source view changes only after Graph confirmation, and the destination retains offline message data. The transaction reconciles a duplicate produced by independently advanced folder deltas. | An uncertain Graph response leaves the source cache unchanged until synchronization. Both folder scopes and the folder-tree replacement need coordinated reservations. |
| Leave the cache unchanged until both delta feeds reconcile | Reuses the existing sync update path and avoids a cache move transaction. | Graph may have confirmed the move while the UI still shows the item in its old folder and the destination cannot open the cached body. |
| Move the cache before Graph confirmation | Updates the view immediately. | A rejected or offline request would make local state disagree with Microsoft and require rollback or a new outbox policy. |
| Add a durable pending-move marker and reconcile it after restart | Preserves an explicit unresolved operation across process restarts. | Adds persistent workflow state and recovery policy for an action the existing startup account sync can reconcile without replaying the write. |

## Consequences

- Message moves are online-only. The app does not send a second move request automatically, including after a 429 or uncertain outcome.
- Account synchronization owns an account-wide reservation and cannot overlap folder updates in that account. Individual message updates reserve their affected folders, so operations in separate folders can proceed concurrently. The native view shows move progress and disables other message actions while the move is active.
- A confirmed move can still need a cache sync if the SQL transaction fails. The native view reports that Graph moved the message and requests synchronization rather than claiming the cache changed.
- An uncertain result may leave stale source metadata until account synchronization completes. The native view asks for sync before retrying. After a restart, the native view keeps moves disabled until account synchronization and its cache snapshot both succeed.
- Synthetic Graph, workflow, persistence, and headless UI tests prove local request construction and state transitions. Issue #34 remains open until a dedicated tenant verifies Graph's move response and both folder delta feeds.

## Sources

- [Microsoft Graph move message v1.0](https://learn.microsoft.com/en-us/graph/api/message-move?view=graph-rest-1.0)
- [Microsoft Graph immutable Outlook identifiers](https://learn.microsoft.com/en-us/graph/outlook-immutable-id)
- [Microsoft Graph message resource v1.0](https://learn.microsoft.com/en-us/graph/api/resources/message?view=graph-rest-1.0)
- [Microsoft Graph permissions reference: Mail.ReadWrite](https://learn.microsoft.com/en-us/graph/permissions-reference?view=graph-rest-1.0#mailreadwrite)
- [ADR 0004: Local data and synchronization](0004-local-data-and-sync.md)
- [ADR 0006: Graph HTTP transport](0006-graph-http-transport.md)
- [ADR 0009: Account-scoped domain identifiers](0009-account-scoped-domain-identifiers.md)
- [Issue #33: Move cached messages between visible folders](https://github.com/1fach/einfach-mail/issues/33)
- [Issue #34: Verify message moves in a Microsoft 365 test tenant](https://github.com/1fach/einfach-mail/issues/34)
