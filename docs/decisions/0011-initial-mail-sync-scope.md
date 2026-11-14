# ADR 0011: Define the initial read-only mail synchronization scope

- Status: Accepted; the message-list and folder metadata bullets are amended by [ADR 0067](0067-message-previews-folder-unread-counts-and-kinds.md)
- Date: 2026-10-31
- Owners: Project maintainer

## Context

M2 needs useful offline folder and message browsing without downloading every message body. Microsoft Graph's message delta query supports a `receivedDateTime` lower-bound filter. Graph encodes initial query options in the returned next and delta links. Filtered delta responses can still include removal and read-state events outside that filter. Message delta is not available in the China-operated-by-21Vianet cloud.

The Linux Secret Service and cache-key lifecycle proof has passed. Issue #8 implements the Linux encrypted cache and sync workflow with synthetic data. Each sync attempt is bounded to 1,000 pages and 64 MiB of response bodies per folder; the page that would exceed the response budget is not committed, so the prior checkpoint remains resumable. The M2 read-only boundary excludes server-side mail changes. No test tenant is available, so the implementation has not been verified against a real mailbox.

Microsoft Graph supports a `receivedDateTime` lower-bound filter on message delta queries, but a filtered delta query returns at most 5,000 messages. The Linux transport and persistence workflows exercise an unfiltered initial delta round across 5,001 synthetic messages and apply the local cutoff. This is synthetic contract evidence, not a real mailbox proof. See [message delta query parameter support](https://learn.microsoft.com/en-us/graph/delta-query-messages#use-query-parameters-in-a-delta-query-for-messages).

## Decision

Use this scope for the first read-only mail milestone:

- Support Microsoft's Global Graph service only. Do not claim mail-sync support for US Government or China-operated-by-21Vianet accounts until every required operation and the matching Entra authority are verified.
- Traverse the signed-in user's non-hidden mail-folder tree. Keep folder metadata and message sync state scoped to the account and exact folder.
- At the account's first mail sync, set the cutoff to 90 days before that sync.
- Synchronize message metadata for each folder when `receivedDateTime` is on or after the cutoff. Keep older messages out of scope, even if a later move places them in a synchronized folder.
- Keep the cutoff fixed for this sync scope. Store it with the account and reuse it when resuming or rebuilding a delta cursor.
- Keep visible folder metadata account-scoped: provider ID, display name, and optional parent-folder ID. Exclude hidden folders from the browsable tree.
- Keep message-list metadata account- and folder-scoped: provider ID, optional subject, `from`, `sender`, received time, and read state. Keep `from` and `sender` distinct because Graph defines different semantics for delegated messages. Do not request `bodyPreview`, message bodies, or attachments in folder listing and delta responses.
- Populate folder message lists from the encrypted cache after delta synchronization. Do not add a parallel provider operation that lists the same full-folder metadata.
- Fetch a message body only after the user opens that message. Store the raw body in the encrypted cache so the user can reopen it offline. Sanitize it at display time. Do not download attachments in M2.
- Keep synchronized message metadata until account removal. Clear an opened body's cached content when a message delta upsert updates its metadata, and clear retained bodies when a replacement scan completes. M2 does not add a rolling-retention setting or a separate history-import control. See [ADR 0034](0034-invalidate-cached-message-bodies-after-delta-updates.md).
- Keep M2 read-only. Do not mark messages read, move, archive, flag, delete, or send mail from the app.

Amendment, 2026-11-14: [ADR 0067](0067-message-previews-folder-unread-counts-and-kinds.md) amends the message-list and folder metadata bullets above. Folders gain a kind and a provider-reported unread count. Messages gain a sanitized preview of at most 256 characters, so the app now requests `bodyPreview`. Message bodies and attachments stay out of folder listing and delta responses.

Treat Graph's next and delta links as opaque and follow the complete chain. A response may contain read-state changes or removal entries for messages outside the local cutoff. Apply those changes to records already in the local scope; do not insert unknown messages older than the cutoff. Updated records can contain only the changed properties, so preserve omitted values and distinguish them from explicit nulls when merging into cached records.

### Initial-sync completeness

Initialize each folder with an unfiltered message delta round. Request only the metadata fields needed by M2, follow every `@odata.nextLink` through the final `@odata.deltaLink`, and apply the fixed cutoff locally. Persist and display only messages on or after the cutoff. Do not store older messages, bodies, or attachments. Apply removals and sparse updates only to messages already in the local scope.

This choice reads the selected metadata for each folder's full history once. It avoids the 5,000-message cap on filtered delta queries and gives the app a complete cursor. The production adapter bounds response and traversal sizes, and the sync workflow commits each page with its restart-safe checkpoint under ADR 0004. Never mark a folder synchronized until all its changes and final delta link are durably committed. The provider's response-size limits are safety budgets, not a production mailbox-size limit.

A filtered message list followed by a separate delta bootstrap could avoid reading older metadata, but the messages documentation does not describe a `latest` delta token. The sequence would also need to prove that concurrent changes cannot fall between the list and cursor. Do not use this alternative unless a test tenant proves a complete cursor handoff. If the one-time metadata scan is unacceptable, revise the product scope before implementation.

Represent metadata changes as a provider-independent `MessageMetadataPatch` in the domain crate. Each field records either no change or a new value. For nullable subject and mailbox fields, an explicit null clears the cached value. A null for `receivedDateTime` or `isRead` is invalid. The Graph adapter maps JSON field presence into this patch. Apply a partial patch only to a known cached message. Application and persistence workflows must not create a complete message from an incomplete patch.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Import all folders, all message history, and all bodies at sign-in | Maximizes offline availability immediately | Large, slow first sync; downloads content the user may never open; stores substantially more sensitive data |
| Synchronize only Inbox metadata for the recent window | Smallest initial network and storage footprint | Other visible folders would have no offline message list, weakening the folder-browsing outcome |
| Synchronize metadata in all visible folders from a fixed 90-day cutoff, then fetch bodies on open | Gives the user a coherent recent mailbox across folders while avoiding an all-history import and eager body downloads | The fixed cutoff and retained cache grow as new mail arrives; older history is not automatically imported |
| Maintain a rolling 90-day window | Bounds message metadata and body retention over time | Moving Graph's date filter requires periodic full folder re-bootstrap and careful cursor replacement and pruning; that cost is deferred until a retention policy is needed |

### Delta patch ownership

The Graph adapter could merge changes directly into cached records, but that would couple cache semantics to Graph JSON. Application workflows could rebuild complete metadata for each change, but then each workflow would own the same field-level merge rules. The domain patch keeps those rules provider-independent while the Graph adapter handles field presence.

## Consequences

- The first sync requests recent metadata for every visible folder. Each folder has its own delta cursor.
- The first sync imports messages received in the prior 90 days. It also imports later messages as they arrive. Mail received before the cutoff is unavailable offline in M2.
- Bodies become available offline only after a user opens them while online. Message HTML remains untrusted and is sanitized every time it is rendered; sanitized output is never persisted.
- The cache stores the account's initial 90-day cutoff, folder cursors, and encrypted metadata and opened bodies. The cutoff remains fixed until account removal. Delta updates invalidate a body's cache entry so the next open fetches current content. Connected-account sign-out removes the local SQLCipher files and key before deleting the saved sign-in; the workflow blocks sign-out if local cache removal fails.
- The app's first mail sync uses the Global Graph authority. A Graph endpoint mapping alone does not establish support for every operation in a national cloud.
- Synthetic transport and persistence tests verify unfiltered pagination with 5,001 messages, cutoff filtering, restart checkpoints, final cursor commits, delta removals, sparse read-state changes, and opened-body cache behavior. The GPUI test verifies offline cached-body access. Live Graph pagination and a full mailbox sync remain tenant-only criteria in issue #8; do not use real mailbox content in automated tests.

## Sources

- [Microsoft Graph message delta](https://learn.microsoft.com/en-us/graph/api/message-delta?view=graph-rest-1.0)
- [ADR 0034: Invalidate cached message bodies after delta updates](0034-invalidate-cached-message-bodies-after-delta-updates.md)
- [Microsoft Graph message delta query parameter support and the filtered-result limit](https://learn.microsoft.com/en-us/graph/delta-query-messages#use-query-parameters-in-a-delta-query-for-messages)
- [Microsoft Graph delta query state-token behavior](https://learn.microsoft.com/en-us/graph/delta-query-overview#state-tokens)
- [Microsoft Graph message resource](https://learn.microsoft.com/en-us/graph/api/resources/message?view=graph-rest-1.0)
- [Microsoft Graph delta response resource representation](https://learn.microsoft.com/en-us/graph/delta-query-overview#resource-representation-in-the-delta-query-response)
- [Microsoft Graph emailAddress resource](https://learn.microsoft.com/en-us/graph/api/resources/emailaddress?view=graph-rest-1.0)
- [Microsoft Graph mailFolder resource](https://learn.microsoft.com/en-us/graph/api/resources/mailfolder?view=graph-rest-1.0)
- [Microsoft Graph list mail folders](https://learn.microsoft.com/en-us/graph/api/user-list-mailfolders?view=graph-rest-1.0)
- [Microsoft Graph list child folders](https://learn.microsoft.com/en-us/graph/api/mailfolder-list-childfolders?view=graph-rest-1.0)
- [Microsoft Graph national cloud deployments](https://learn.microsoft.com/en-us/graph/deployments)
- [Local cache and synchronization decision](0004-local-data-and-sync.md)
- [Graph transport decision](0006-graph-http-transport.md)
