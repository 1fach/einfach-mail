# ADR 0040: Move messages to the default Archive folder

- Status: Accepted
- Date: 2026-11-04
- Owners: Einfach Mail maintainers

## Context

Issue #37 adds a native Archive action for an opened cached message. It means the default Archive folder used by Outlook's One_Click Archive feature. It does not mean the separate Exchange Online Archive Mailbox. The Archive folder's displayed name can be localized, and the application must use its actual account-scoped folder ID for cache reconciliation and folder navigation.

Issue #35 has already added a Delete workflow that resolves Deleted Items and then reserves, confirms, and reconciles the move. Adding Archive as a second independent resolver and workflow would duplicate the same account-sync, source-cache, visibility, reservation, exact-confirmation, and encrypted-cache rules. Microsoft Graph defines both `archive` and `deleteditems` as locale-independent well-known folder names and permits a move request to target either a folder ID or a well-known name. Einfach Mail still needs the resolved ID to validate against the visible cached folder tree and open Archive after confirmation.

## Decision

- Add an application-owned `WellKnownMailFolder` value with `Archive` and `DeletedItems` variants. The Microsoft Graph adapter maps these variants to Graph well-known names and returns a `FolderReference` scoped to the verified active account. The application checks that reference against its visible cached folder tree.
- Replace the Delete-only application workflow entry point with one `move_message_to_well_known_folder` operation. Keep Archive and Delete as distinct native actions that select the typed destination.
- The application workflow owns the shared safety rules: require a successful sync in the session, reserve the source while resolving the destination, verify the cached source and visible destination, reserve both folders, require exact Graph confirmation, and reconcile the encrypted message row and body transactionally.
- Return the resolved destination reference and cache result after provider confirmation. The Archive view offers an `Open Archive` action after successful cache reconciliation; the UI does not infer the folder from a localized display name or call the provider directly.
- Reject a source already in its selected destination. Preserve the existing confirmed, uncertain, canceled, and cache-failure recovery behavior. Do not queue actions offline or retry an uncertain move.
- Keep Archive limited to the default Outlook folder. Do not add Exchange Online Archive Mailbox support, an unarchive action, a dependency, or a cache schema change. Keep consent and live service verification in issue #38.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Resolve both destinations through one typed application port and one move workflow | Keeps Graph names at the adapter boundary and gives Archive and Delete one owner for synchronization, reservation, confirmation, and cache rules. | Requires migrating the internal Delete caller and updating the shared outcome to carry its resolved destination. |
| Add separate Archive and Delete provider methods and duplicate each workflow | Makes each method name explicit and keeps each diff locally small. | Copies the same safety and recovery rules, allowing the two destructive actions to drift. |
| Send Graph's `archive` well-known name directly in the move request | Avoids a folder lookup. | Does not validate the actual destination against the synchronized visible tree and does not provide its folder ID for navigation. |

## Consequences

- The UI can offer to open the exact Archive folder returned for the active account, independent of locale.
- A provider rejection leaves the encrypted cache unchanged. An uncertain or canceled move, or confirmed server success followed by cache failure, requires a successful account sync before another special-folder action.
- The scope is the user's primary mailbox Archive folder. Graph documentation explicitly distinguishes it from the in-place Exchange Online Archive Mailbox.
- Synthetic Graph, workflow, persistence, and headless GPUI tests verify the resolution, ordering, reservations, confirmation, recovery, encrypted body preservation, and navigation target. Issue #38 remains open for tenant consent and live Graph behavior.

## Sources

- [Microsoft Graph mailFolder resource v1.0](https://learn.microsoft.com/en-us/graph/api/resources/mailfolder?view=graph-rest-1.0)
- [Microsoft Graph move message v1.0](https://learn.microsoft.com/en-us/graph/api/message-move?view=graph-rest-1.0)
- [Microsoft Graph Outlook mail API overview](https://learn.microsoft.com/en-us/graph/api/resources/mail-api-overview?view=graph-rest-1.0)
- [ADR 0039: Move messages to Deleted Items](0039-move-messages-to-deleted-items.md)
- [Issue #37: Add native Archive action for cached messages](https://github.com/1fach/einfach-mail/issues/37)
- [Issue #38: Verify Archive action in a Microsoft 365 test tenant](https://github.com/1fach/einfach-mail/issues/38)
