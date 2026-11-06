# ADR 0034: Invalidate cached message bodies after delta updates

- Status: Accepted for Linux implementation
- Date: 2026-11-03
- Owners: Einfach Mail maintainers

## Context

Einfach Mail requests message metadata in folder delta queries and fetches a body only when a user opens a message. Graph can report a message update without returning every changed property. Because the delta projection omits the body, a cached body can become stale while its metadata is updated.

The cache also supports generation-tagged replacement scans after an expired delta cursor. It keeps cached rows visible and the scan resumable while Graph returns replacement pages.

## Decision

Clear a cached message body when any delta upsert updates its metadata row, including a page in an expired-cursor replacement scan. Commit the invalidation in the same SQLCipher transaction as the delta page and checkpoint.

During a replacement scan, keep existing cached rows available while the scan is incomplete. A message keeps its prior metadata and body until Graph returns its replacement upsert. That page updates the metadata and clears the body's cached content together. When the final page arrives, delete unseen messages and clear bodies for all retained messages in the same transaction that saves the replacement delta link. The next open fetches and caches the current body.

Within one mail session, serialize a missing-body fetch and cache write against synchronization of the same folder. Reserve the folder before fetching, then reread the message after acquiring the reservation. This prevents a delta update in that session from committing between the fetch and body write and leaving stale content cached after the cursor advances.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Invalidate bodies on message delta upserts and after a completed replacement scan | Do not show an older cached body after Graph reports a message update. Keep the metadata-only delta projection and lazy body fetch. | The user must reopen an updated message online to make its body available offline again. |
| Include bodies in message delta queries | The cache can replace body and metadata together. | Downloads message content during folder sync, including messages the user never opens, and increases response size. |
| Keep cached bodies until account removal | Preserves offline body access without another request. | Can show content that no longer matches the updated message. |

## Consequences

- An ordinary or replacement-scan delta upsert clears the stored body for an existing message, even when the selected metadata values did not change. This also handles updates that change only the omitted body.
- During an interrupted replacement scan, rows not yet returned by Graph retain their prior metadata and bodies. Rows already updated by a replacement upsert have no cached body. A completed replacement scan clears bodies for every retained message, including rows whose metadata did not change.
- A missing-body fetch holds its session's folder reservation through the cache write. If synchronization in that session already owns the folder, opening the body returns the existing synchronization-in-progress error and can be retried.
- The application fetches and stores a current body the next time the user opens a message with no cached body.
- Synthetic SQLCipher and workflow regressions cover ordinary delta invalidation, replacement-scan behavior, and synchronization serialization. No live Graph request or desktop window is required for these local rules.

## Sources

- [Microsoft Graph message delta v1.0](https://learn.microsoft.com/en-us/graph/api/message-delta?view=graph-rest-1.0)
- [Microsoft Graph message resource](https://learn.microsoft.com/en-us/graph/api/resources/message?view=graph-rest-1.0)
- [ADR 0011: Initial read-only mail synchronization scope](0011-initial-mail-sync-scope.md)
- [ADR 0032: Recover expired message delta cursors](0032-recover-expired-message-delta-cursors.md)
- [Issue #8: Browse folders and read synchronized mail offline](https://github.com/1fach/einfach-mail/issues/8)
