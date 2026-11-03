# ADR 0037: Reject stale mail sync pages

- Status: Accepted
- Date: 2026-11-03
- Owners: Einfach Mail maintainers

## Context

`MailWorkflowFactory` can open more than one session for the same account. Each session can open its own cache connection. A provider request can therefore remain in flight while another session advances the same folder's delta checkpoint or starts a replacement generation. Account synchronization also lists and replaces the folder snapshot before it acquires each folder reservation. A stale concurrent folder listing can delete cached folders, messages, and checkpoints while another session is syncing.

Persistence currently applies a returned page using whichever replacement generation is active when the page commits. A delayed response can consequently enter a newer snapshot or finalize it with a cursor fetched from an older checkpoint. A page transaction must prove that its request began from the checkpoint it is about to advance.

## Decision

Share account and folder reservations among sessions created by one `MailWorkflowFactory`. An account synchronization acquires its account reservation before listing folders and holds it through folder replacement and delta requests. The account reservation conflicts with folder operations for that account. Keep standalone `MailWorkflow` reservations local to that workflow.

Persist a monotonically increasing checkpoint revision with each folder's sync state. Pass the state used to build each provider request into the page-commit and reset-start cache operations. Each operation must compare that expected state and revision to the database state inside the same transaction, then advance the revision atomically with the cursor and page changes. Reject stale operations without applying their changes.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Share account and folder reservations across factory sessions only | Prevents stale folder listings and overlapping folder operations in the normal application composition root | Does not protect separate factory instances or callers that construct workflows directly; persistence cannot identify a delayed response |
| Validate an expected checkpoint only in persistence | Enforces the cursor transition at the shared database boundary | Allows avoidable duplicate requests and does not serialize body fetches against syncs from sessions created by one factory |
| Share factory reservations and validate a persisted checkpoint revision | Prevents routine overlaps and rejects stale writes across workflow and cache instances | Adds a checkpoint revision to the cache schema and a concurrency error for callers to present |

## Consequences

- A folder sync page or reset response can advance only the exact checkpoint revision from which its request was sent.
- Sessions created by one factory return `OperationInProgress` when another session is synchronizing the account or operating on one of its folders.
- A stale result from another workflow or factory returns a cache checkpoint-conflict error and leaves the newer checkpoint intact.
- The Linux synthetic regression suite covers stale page commits, stale reset responses, stale folder listings, and multiple sessions from one factory. Live Graph behavior remains unverified under issue #8.

## Sources

- [ADR 0012: Keep mail sessions behind application workflows](0012-application-owned-mail-sessions.md)
- [ADR 0032: Recover expired message delta cursors](0032-recover-expired-message-delta-cursors.md)
- [Microsoft Graph delta query overview](https://learn.microsoft.com/en-us/graph/delta-query-overview)
