# ADR 0047: Share write readiness across account sessions

- Status: Accepted
- Date: 2026-11-07
- Owners: Einfach Mail maintainers

## Context

ADR 0043 allows one workflow factory to open several sessions for an account. Those sessions need shared mail sync readiness and shared knowledge of which calendar views completed a sync in the current factory.

Calendar event updates and deletions also need account-wide recovery that survives dropped futures and process restarts. ADR 0052 assigns that recovery to SQLCipher markers. This decision covers only the in-memory readiness shared by sessions from one factory.

## Decision

Share in-memory readiness among sessions opened for the same account by one `AccountSessionWorkflowFactory`. Keep mail sync readiness and calendar view readiness as separate policies. Key recovery by `AccountId` with weak references, and prune entries without live workflow owners when opening another session.

Record a calendar view as ready only after it completes a sync in that factory. A new factory starts each view as unsynchronized, even when SQLCipher has a saved cursor. Use the durable account-wide recovery markers and their transaction rules from ADR 0052 for event updates and deletions.

The registry does not keep readiness state alive after the last workflow drops. A later session open prunes its weak entry and creates fresh state. A process restart also starts with each calendar view unsynchronized.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Share readiness in the account-session factory | Closes sibling-session gaps without adding readiness fields to the cache; retains separate mail and calendar rules. | Coordinates only sessions created by the same factory, which is the application composition boundary. |
| Persist per-factory mail and calendar-view readiness | Lets new factories reuse completed sync state. | Adds separate persisted readiness state and lifecycle rules. Calendar post-write recovery markers are already covered by ADR 0052. |
| Permit only one session per account | Avoids shared readiness state. | Conflicts with ADR 0043 and persistence's supported multiple cache clients. |

## Consequences

- Sessions opened by one factory agree about mail sync readiness and which calendar views have synced in that factory.
- A new factory requires a successful sync of each calendar view before it permits a write.
- ADR 0052 owns the SQLCipher schema and recovery-marker lifecycle for calendar updates and deletions. This decision adds no cache schema or Graph request changes.
- Synthetic workflow tests verify shared readiness. Live Microsoft Graph behavior remains unverified.

## Sources

- [ADR 0040: Move messages to the default Archive folder](0040-move-messages-to-the-default-archive-folder.md)
- [ADR 0043: Use one per-account mail and calendar session](0043-per-account-mail-and-calendar-session.md)
- [ADR 0046: Use one account cache removal port](0046-account-cache-removal-port.md)
- [ADR 0052: Persist calendar event write recovery](0052-persist-calendar-event-write-recovery.md)
- [Issue #102: Share uncertain-write recovery across account sessions](https://github.com/1fach/einfach-mail/issues/102)
