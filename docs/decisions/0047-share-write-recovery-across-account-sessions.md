# ADR 0047: Share write recovery across account sessions

- Status: Accepted
- Date: 2026-11-07
- Owners: Einfach Mail maintainers

## Context

ADR 0043 allows one workflow factory to open several sessions for an account. The factory shares operation reservations, but mail sync readiness and calendar write recovery live in each workflow. One session can therefore report an uncertain remote write while another session still allows a write from stale state.

Calendar updates and deletions also lack persisted operation markers. If a caller drops either future while Graph or the cache is handling the write, the reservation is released without recording that the result may need reconciliation.

## Decision

Share in-memory write recovery among sessions opened for the same account by one `AccountSessionWorkflowFactory`. Keep mail account readiness and calendar view recovery as separate policies. Key recovery by `AccountId` with weak references, and prune entries without live workflow owners when opening another session.

Guard calendar updates and deletions from the start of the provider request through cache reconciliation. A dropped future, uncertain result, stale event, mismatched confirmation, or confirmed write that the cache cannot commit marks the account's previously synchronized calendar views for sync. A definitive provider rejection and a completed cache reconciliation finish the guard without adding a recovery requirement.

New workflows start with mail sync required and calendar views unsynchronized. The registry does not keep recovery state alive after the last workflow drops. A later session open prunes its weak entry and creates fresh state; a process restart also requires sync before writes.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Share recovery state in the account-session factory | Closes sibling-session gaps without changing the cache schema; retains separate mail and calendar rules. | Coordinates only sessions created by the same factory, which is the application composition boundary. |
| Persist mail and calendar recovery markers | Coordinates separate factories and processes. | Adds schema and recovery-marker lifecycle changes even though the desktop permits one process and a new session already starts unsynchronized. |
| Permit only one session per account | Avoids shared readiness state. | Conflicts with ADR 0043 and persistence's supported multiple cache clients. |

## Consequences

- Sessions opened by one factory agree when a mail or calendar write needs synchronization.
- Cancellation after a calendar write starts blocks later calendar writes until the affected views sync.
- A process restart clears in-memory recovery state, but new sessions cannot write until they sync. Entries for dropped workflows are pruned when the factory next opens a session.
- The cache schema and Graph request behavior do not change.
- Synthetic workflow and persistence tests verify the shared state and cancellation paths. Live Microsoft Graph behavior remains unverified.

## Sources

- [ADR 0040: Move messages to the default Archive folder](0040-move-messages-to-the-default-archive-folder.md)
- [ADR 0043: Use one per-account mail and calendar session](0043-per-account-mail-and-calendar-session.md)
- [ADR 0046: Use one account cache removal port](0046-account-cache-removal-port.md)
- [Issue #102: Share uncertain-write recovery across account sessions](https://github.com/1fach/einfach-mail/issues/102)
