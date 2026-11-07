# ADR 0046: Use one account cache removal port

- Status: Accepted
- Date: 2026-11-07
- Owners: Einfach Mail maintainers

## Context

Mail and calendar data share one encrypted account cache. Account removal checks both the unresolved mail-send marker and the unresolved calendar-event marker before deleting the cache files and key. The operation is currently exposed by both feature cache ports, with mail and calendar error types translating the same account-wide outcome into feature-specific variants.

The retained account session and the unopened-session factory are both required removal paths. The session path shares reservations and the cache worker with active workflows; the factory path is used only when an account session was never opened. ADRs 0027 and 0043 require these lifecycle guarantees.

## Decision

Add one application-owned `AccountCache` port for account-wide cache removal and one `AccountCacheRemovalError` for its outcomes. Remove account removal from the mail and calendar feature cache ports. Require the shared cache handle supplied by `AccountCacheFactory` to implement `AccountCache`.

Keep `AccountSession::remove_account_cache` and `AccountSessionFactory::remove_account_cache` as the two application entry points. The session implementation calls the shared cache port while holding the shared account reservation. The factory implementation reserves the account and delegates to the factory's unopened-cache removal path. Persistence implements both paths through the same account-wide removal rules.

The shared error distinguishes unavailable storage, a missing key, invalid stored state, account mismatch, active cache use, an active application operation, an unresolved mail send, and an unresolved calendar event creation. Feature cache errors no longer contain the other feature's removal state.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Keep removal on both feature cache ports | No interface migration. | Duplicates one account-wide operation and keeps feature-specific error translation. |
| Add one account cache lifecycle port | Gives account-wide data and marker checks one owner while preserving session and factory paths. | Requires updating persistence, application sessions, and desktop removal error handling. |
| Put removal only on the cache factory | Keeps the cache interface smaller. | Cannot reuse the retained session's worker and reservation on sign-out, which can race queued cache work. |

## Consequences

- Mail and calendar cache ports own only their feature data operations.
- One lifecycle error type represents account removal regardless of whether a retained session or unopened factory performed it.
- Pending-operation checks, account reservations, lock ordering, retry behavior, database deletion, and credential deletion remain unchanged.
- Removal remains a local cache operation; this decision makes no live provider behavior claim.

## Sources

- [ADR 0027: Serialize mail account removal with mail operations](0027-serialize-mail-account-removal-with-mail-operations.md)
- [ADR 0043: Use one per-account mail and calendar session](0043-per-account-mail-and-calendar-session.md)
- [Issue #84: Untangle shared ownership in the Rust workspace](https://github.com/1fach/einfach-mail/issues/84)
