# ADR 0049: Use account-scoped errors when opening the shared cache

- Status: Accepted
- Date: 2026-11-07
- Owners: Einfach Mail maintainers

## Context

Mail and Calendar open the same account cache through `AccountSessionFactory`. The `AccountCacheFactory` and `AccountSessionFactory` ports currently return `MailCacheError` for cache-open failures. AccountView uses that result for either child view and maps it through mail-specific error copy. A Calendar cache-open failure can therefore be reported as a mail cache failure.

ADR 0046 gives account-wide cache removal its own port and error type. Opening the shared cache is another account-wide lifecycle boundary and should not expose a mail operation error.

## Decision

Define `AccountCacheOpenError` in the application crate for failures opening the shared account cache. Use it as the result error for `AccountCacheFactory::open_for_account` and `AccountSessionFactory::open_session`. Keep `MailCacheError` for mail operations. The persistence adapter maps cache initialization failures to the account-scoped error at its boundary, and AccountView presents the same account-cache recovery message for Mail and Calendar.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Keep `MailCacheError` and change only the error copy | Minimal interface change. | The account boundary still exposes a mail-specific error and forces account UI code to depend on mail terminology. |
| Reuse `AccountCacheRemovalError` for opening | Avoids another error type. | Opening cannot produce removal-only outcomes such as pending mail sends or calendar creations. The type would admit impossible states. |
| Add `AccountCacheOpenError` | Gives account cache creation one accurate error contract while leaving mail workflow errors unchanged. | Requires updating the account-session port, persistence adapter, desktop view, and synthetic implementations. |

## Consequences

- Account-wide cache opening and mail operations have separate error contracts.
- Mail and Calendar show account-level guidance when the shared cache cannot open.
- This changes no stored data, cache-key behavior, Graph request, or cache lifecycle ordering.
- Implementations and tests of the public account-session ports must handle the new error type.

## Sources

- [Issue #130: Keep account cache-open errors feature-neutral](https://github.com/1fach/einfach-mail/issues/130)
- [ADR 0043: Use one per-account mail and calendar session](0043-per-account-mail-and-calendar-session.md)
- [ADR 0046: Use one account cache removal port](0046-account-cache-removal-port.md)
