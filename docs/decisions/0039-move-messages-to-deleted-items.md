# ADR 0039: Move messages to Deleted Items

- Status: Accepted
- Date: 2026-11-04
- Owners: Einfach Mail maintainers

## Context

Issue #35 adds a native Delete action for an opened cached message. Deletion must preserve the encrypted offline body and use Graph's move action so the message remains recoverable in Deleted Items. Graph folder names shown to users are localized, so the application cannot identify the destination by its display name. The account's visible folder tree is cached from an earlier sync, while the provider can resolve the current well-known folder name.

The existing `move_message` workflow reserves both known folders, requires an exact Graph move confirmation, and moves the cached row and body in one transaction. A Delete action must discover its destination before it can reserve both folders. Account synchronization already conflicts with any folder reservation and replaces the visible folder tree only while holding an account reservation.

Microsoft Graph v1.0 supports well-known mail folder names. The `deleteditems` name resolves independently of mailbox locale; `GET /me/mailFolders/deleteditems` returns the provider folder record. The returned ID can be compared to the active account's cached visible folder tree before any message write.

## Decision

- Add an application-owned `delete_message` workflow and a `MailProvider` operation that resolves the signed-in account's Deleted Items folder ID. Keep the well-known name and Graph JSON in the Microsoft Graph adapter.
- Require a successful account sync in the current mail session before `move_message` or `delete_message` can write. Clear that readiness when sync starts and restore it only after all folder pages complete. Require another sync after an uncertain move, a canceled move future, or a confirmed move whose cache transaction fails.
- Reserve the source folder before resolving Deleted Items. Keep that reservation while the provider resolves the well-known folder, construct an account-scoped destination reference, reject a source that already matches Deleted Items, reserve the destination, and verify both references belong to the visible cached folder tree.
- Require the source message to remain cached. Reuse the existing provider move and cache transaction after these checks. Require Graph's exact `201 Created` confirmation with the same immutable message ID and the resolved destination ID before rehoming the encrypted cache row and body.
- Show a native `Delete` button for an opened cached message. Report pending, confirmed, and failed outcomes. Route a Graph `Mail.ReadWrite` denial through the account view's delegated permission review without changing the cache. Keep the source cached after an uncertain result and require a successful account sync before another move attempt.
- Do not call Graph `permanentDelete`, add a dependency, or change the cache schema. Keep live tenant verification in issue #36.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Resolve the well-known folder ID, validate it against the cached tree, then reuse the move workflow | Uses a locale-independent destination and existing Graph confirmation and encrypted cache transaction. | Adds one provider read before the move and requires holding the source reservation while resolving the destination. |
| Find the destination by its display name | Reuses the cached folder list without another request. | Display names are localized and users can create folders with the same name. |
| Call Graph `permanentDelete` | Removes the message without a move. | Permanently deletes data and violates the product's recoverable Delete behavior. |
| Hold an account-wide reservation for resolution and move | Prevents all account operations while the destination is resolved. | Blocks unrelated folder work; source and destination reservations already prevent sync and serialize only affected folders. |

## Consequences

- The provider resolves the locale-independent Deleted Items ID; the application decides whether that ID is valid in the synchronized folder tree.
- The source reservation prevents folder-tree replacement while the destination is being resolved. The destination reservation prevents concurrent writes to the destination through cache reconciliation.
- A provider rejection leaves the encrypted cache unchanged. An uncertain or canceled move leaves the source row and body in place, disables subsequent moves in the current session, and requires a successful account sync.
- Exact remote confirmation followed by a cache failure reports remote success and requires account sync. Successful cache reconciliation preserves the encrypted body and handles an existing destination row through the existing transaction.
- Synthetic tests verify the Graph resolver, application ordering, reservation conflicts, exact move confirmation, cache transaction, and native state. Issue #36 remains open for tenant consent and live mailbox evidence.

## Sources

- [Microsoft Graph mailFolder resource v1.0](https://learn.microsoft.com/en-us/graph/api/resources/mailfolder?view=graph-rest-1.0)
- [Microsoft Graph get mailFolder v1.0](https://learn.microsoft.com/en-us/graph/api/mailfolder-get?view=graph-rest-1.0)
- [Microsoft Graph move message v1.0](https://learn.microsoft.com/en-us/graph/api/message-move?view=graph-rest-1.0)
- [ADR 0020: Move cached messages between visible folders](0020-move-cached-messages-between-folders.md)
- [Issue #35: Move cached messages to Deleted Items](https://github.com/1fach/einfach-mail/issues/35)
- [Issue #36: Verify message moves in a Microsoft 365 test tenant](https://github.com/1fach/einfach-mail/issues/36)
