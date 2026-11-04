# ADR 0012: Keep mail sessions behind application workflows

- Status: Superseded by ADR 0038
- Date: 2026-11-01
- Owners: Project maintainer

## Context

The M2 account view opened `SqliteMailCache` directly and constructed a generic `MailWorkflow` before creating the mail view. The view also removed cache data during sign-out. This contradicted the accepted dependency boundary: GPUI views call application workflows and do not access persistence or provider adapters.

The application workflow is generic over provider and cache implementations. The desktop view needs a stable session API so it can open, synchronize, read, and close a mail session without naming those concrete types. The application also needs a cache-factory port to open or remove one account's cache without depending on SQLCipher.

## Decision

Keep mail session operations and cache creation behind application-owned ports. `MailWorkflow` implements the object-safe `MailSession` interface. `MailWorkflowFactory` combines a provider with a `MailCacheFactory` and returns a type-erased session to the desktop. Persistence implements `MailCacheFactory`; the desktop composition root wires the Graph provider and persistence adapter. GPUI views receive only application workflows and domain values.

The session interfaces return boxed `Send` futures from the standard library. This erases the concrete provider and cache workflow at the UI boundary without changing the generic provider/cache ports or adding an async-trait dependency.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Keep `SqliteMailCache` construction and deletion in the account view | Fewer types and direct method calls | The view depends on persistence and owns adapter lifecycle, contrary to the accepted system boundary |
| Make all provider and cache ports object-safe | One workflow could hold trait objects directly | Changes every provider/cache implementation and broadens type erasure beyond the UI boundary |
| Use a general async-trait dependency | Concise object-safe async methods | Adds a dependency for a narrow seam that the standard `Future`, `Pin`, and `Box` types express directly |

## Consequences

- Mail views depend on application session methods and do not name Graph or SQLite types.
- The desktop entry point remains the composition root for concrete adapters.
- Cache creation, removal, synchronization, and message reads stay in application workflows and persistence adapters.
- The Linux candidate must pass dependency-boundary checks, workspace tests, Clippy, and Rustdoc. Live mailbox verification remains under issues #3 and #8.

## Sources

- [System architecture](../architecture/system.md)
- [ADR 0004: Keep a local cache and synchronize with Graph](0004-local-data-and-sync.md)
- [ADR 0005: Use SQLCipher-backed SQLite for the local cache](0005-encrypted-local-cache.md)
