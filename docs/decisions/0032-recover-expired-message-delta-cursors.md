# ADR 0032: Recover expired message delta cursors

- Status: Accepted for Linux implementation; live Graph verification remains open
- Date: 2026-11-02
- Owners: Einfach Mail maintainers

## Context

Microsoft Graph can invalidate a message delta token. The documented reset response is `410 Gone` with a `Location` URL that contains an empty `$deltatoken`. The client must replace its local folder snapshot with a full synchronization. Outlook delta-token lifetime has no fixed upper limit.

Einfach Mail commits each message page and its continuation link to SQLCipher. A reset must survive process exit. Clearing a folder before the replacement scan finishes would hide usable mail and discard cached bodies if Graph or the network fails.

## Decision

Return a reset result only for message delta requests that receive `410 Gone`. Require the `Location` URL to use the configured Graph authority, target the requested folder's message delta path, and contain the initial message projection with an empty `$deltatoken`. Store the validated URL as the resumable next link.

Start a replacement generation in one cache transaction. Preserve the current delta link and cached messages. Mark each message returned by the replacement pages with the active generation. During the rebuild, treat removals and cutoff exclusions as unseen records instead of deleting them. Commit each page and its next link atomically.

When the final page arrives, delete messages that were not seen in the replacement generation, clear the remaining generation markers, and save the new delta link in the same transaction. If the process stops before the final page, resume from the saved link and keep the previous snapshot available. Show rebuild progress in the native mail view.

Allow one reset response per synchronization attempt. If Graph resets the replacement cursor again, save its new reset link and return a safe error. The next attempt resumes from that checkpoint.

## Alternatives

| Option | Benefit | Cost |
| --- | --- | --- |
| Keep the old snapshot until a generation-tagged replacement completes | Offline mail and cached bodies remain available after interruptions. The final replacement commits atomically. | Adds generation state and reconciliation logic to SQLCipher. |
| Delete the cached folder when Graph resets its cursor | Avoids replacement-generation tracking. | A failed scan removes usable offline mail and bodies. |
| Return the Graph error and require the user to clear the cache | Keeps reset handling out of the workflow. | Does not recover automatically and asks users to delete valid local data. |

## Consequences

- The application distinguishes a message page from a delta reset. Other Graph operations keep their existing status handling.
- A persistence migration adds active-generation state to folder checkpoints and a generation marker to cached messages. The desktop uses the existing Tokio workspace dependency for its progress channel.
- The workflow bounds reset attempts, page traversal, and response bytes. The provider validates the reset URL before it uses the signed-in user's token.
- Synthetic Graph and SQLCipher tests cover reset mapping, URL validation, interruption, restart, cached-body retention, repeated resets, and final reconciliation.
- No Microsoft 365 test tenant is available. Issue #8's full initial-sync proof remains open.

## Sources

- [Microsoft Graph delta query overview](https://learn.microsoft.com/en-us/graph/delta-query-overview#synchronization-reset)
- [ADR 0004: Local cache and synchronization](0004-local-data-and-sync.md)
- [ADR 0011: Initial mail synchronization scope](0011-initial-mail-sync-scope.md)
