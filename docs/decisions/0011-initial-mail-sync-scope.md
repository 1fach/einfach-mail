# ADR 0011: Define the initial read-only mail synchronization scope

- Status: Accepted
- Date: 2026-10-31
- Owners: Project maintainer

## Context

M2 needs useful offline folder and message browsing without downloading every message body. Microsoft Graph's message delta query supports a `receivedDateTime` lower-bound filter. Graph encodes initial query options in the returned next and delta links. Filtered delta responses can still include removal and read-state events outside that filter. Message delta is not available in the China-operated-by-21Vianet cloud.

The project has accepted an encrypted local cache, but the current Linux credential and cache-key proofs must pass before storing real mailbox content. The M2 read-only boundary also excludes server-side mail changes.

Microsoft Graph supports a `receivedDateTime` lower-bound filter on message delta queries, but a filtered delta query returns at most 5,000 messages. The Linux transport proof now exercises an unfiltered initial delta round across more than 5,000 synthetic messages. This proves pagination at the transport boundary; the production sync and persistence workflow is not implemented. See [message delta query parameter support](https://learn.microsoft.com/en-us/graph/delta-query-messages#use-query-parameters-in-a-delta-query-for-messages).

## Decision

Use this scope for the first read-only mail milestone:

- Support Microsoft's Global Graph service only. Do not claim mail-sync support for US Government or China-operated-by-21Vianet accounts until every required operation and the matching Entra authority are verified.
- Traverse the signed-in user's non-hidden mail-folder tree. Keep folder metadata and message sync state scoped to the account and exact folder.
- At the account's first mail sync, set the cutoff to 90 days before that sync.
- Synchronize message metadata for each folder when `receivedDateTime` is on or after the cutoff. Keep older messages out of scope, even if a later move places them in a synchronized folder.
- Keep the cutoff fixed for this sync scope. Store it with the account and reuse it when resuming or rebuilding a delta cursor.
- Keep visible folder metadata account-scoped: provider ID, display name, and optional parent-folder ID. Exclude hidden folders from the browsable tree.
- Keep message-list metadata account- and folder-scoped: provider ID, optional subject, `from`, `sender`, received time, and read state. Keep `from` and `sender` distinct because Graph defines different semantics for delegated messages. Do not request `bodyPreview`, message bodies, or attachments in folder listing and delta responses.
- Fetch a message body only after the user opens that message. Store the raw body in the encrypted cache so the user can reopen it offline. Sanitize it at display time. Do not download attachments in M2.
- Keep synchronized message metadata and opened bodies until account removal. M2 does not add a rolling-retention setting or a separate history-import control.
- Keep M2 read-only. Do not mark messages read, move, archive, flag, delete, or send mail from the app.

Treat Graph's next and delta links as opaque and follow the complete chain. A response may contain read-state changes or removal entries for messages outside the local cutoff. Apply those changes to records already in the local scope; do not insert unknown messages older than the cutoff. Updated records can contain only the changed properties, so preserve omitted values and distinguish them from explicit nulls when merging into cached records.

### Initial-sync completeness

Initialize each folder with an unfiltered message delta round. Request only the metadata fields needed by M2, follow every `@odata.nextLink` through the final `@odata.deltaLink`, and apply the fixed cutoff locally. Persist and display only messages on or after the cutoff. Do not store older messages, bodies, or attachments. Apply removals and sparse updates only to messages already in the local scope.

This choice reads the selected metadata for each folder's full history once. It avoids the 5,000-message cap on filtered delta queries and gives the app a complete cursor. The production adapter and sync workflow must bound per-page memory and define restart-safe progress under ADR 0004 before implementation. Never mark a folder synchronized until all its changes and final delta link are durably committed. The proof's 64 MiB cumulative-response ceiling is a test bound, not a production mailbox limit.

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
- The cache starts with 90 days of message metadata and grows with later messages and bodies the user opens. Encryption, cache removal, and account-removal behavior remain release gates under ADR 0005 and issues #4 and #20.
- The app's first mail sync uses the Global Graph authority. A Graph endpoint mapping alone does not establish support for every operation in a national cloud.
- The Linux Graph transport proof verifies unfiltered initial delta pagination with 5,001 synthetic messages. The M2 workflow must still prove local cutoff filtering, restart-safe incremental persistence, final cursor commits, delta removals, read-state changes, and the opened-body cache path. Do not use real mailbox content in tests.

## Sources

- [Microsoft Graph message delta](https://learn.microsoft.com/en-us/graph/api/message-delta?view=graph-rest-1.0)
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
