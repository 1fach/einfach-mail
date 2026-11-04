# ADR 0038: Use one per-account mail and calendar session

- Status: Accepted
- Date: 2026-11-04
- Owners: Project maintainer

## Context

ADR 0012 introduced an application-owned, type-erased mail session so GPUI views do not construct Graph providers or SQLCipher caches. Calendar synchronization uses the same signed-in account and the same encrypted per-account database. Separate mail and calendar factories would open independent cache handles and give the account view two session lifecycles to stop before removing one cache.

The application still needs concrete, testable mail and calendar workflows. Only the desktop-facing session boundary needs to erase provider and cache types. The session must keep removal, synchronization, and offline reads behind application ports.

## Decision

Replace the mail-only session boundary with an application-owned `AccountSession` and `AccountSessionFactory`. One account session exposes mail and calendar operations and removes the account cache. The factory opens the cache once, then composes mail and calendar workflows over handles to that same account cache and provider.

The account cache factory requires a cloneable cache handle whose clones refer to the same account cache. Persistence implements that contract for `SqliteMailCache`; synthetic adapters use shared state. The session factory shares synchronization reservations across sessions created by that factory. GPUI views receive only `AccountSession` and domain values. The desktop composition root remains responsible for connecting the concrete Graph provider and persistence adapter.

The interface continues to return boxed `Send` futures from the standard library. Provider and cache ports remain generic and are not made object-safe; no async-trait dependency is added.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| One account session composes both workflows over the shared cache | One lifecycle and one removal boundary; views use a single provider-neutral application port. | The session interface includes both mail and calendar capabilities, and the cache handle must preserve shared identity when cloned. |
| Separate mail and calendar session factories | Each API lists only its own operations. | Opens duplicate cache handles and splits operation reservations and sign-out lifecycle for one account. |
| Let each view open a cache or construct its workflow | Fewer application types. | Gives views persistence/provider dependencies and bypasses the accepted architecture boundary. |
| Make provider and cache ports object-safe | Could store trait objects directly in one workflow. | Broadens type erasure into infrastructure and changes every adapter for a UI-specific need. |

## Consequences

- Mail and calendar views use one account-owned session and do not name Graph or SQLite types.
- Cache creation and removal remain application and persistence responsibilities.
- The session composes separate workflows, so mail and calendar domain logic remain independently testable.
- Shared-cache behavior is verified through cloneable synthetic adapters and persistence integration tests.
- ADR 0012's mail-only session API is superseded; its dependency-boundary rationale remains in force.

## Sources

- [ADR 0012: Keep mail sessions behind application workflows](0012-application-owned-mail-sessions.md)
- [ADR 0014: Implement a cached Linux read-only primary calendar](0014-linux-read-only-calendar-workflow.md)
- [System architecture](../architecture/system.md)
