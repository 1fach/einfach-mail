# ADR 0009: Use account-scoped opaque domain identifiers

- Status: Accepted
- Date: 2026-10-30
- Owners: Project maintainer

## Context

Mail, folder, calendar, and event workflows need stable references that survive pagination, synchronization, and offline use. The domain must not depend on Graph JSON or a database schema. Microsoft Graph IDs are opaque, case-sensitive values. Standard Outlook item IDs can change when an item moves, while Graph's optional immutable ID remains stable across folder moves within one mailbox.

The first provider is Microsoft Graph for Exchange Online. The product has not accepted a second provider or cross-provider item identity. A local cache is accepted, but its database and key design remain open in ADR 0005.

## Decision

Represent account, folder, message, calendar, and event IDs as distinct opaque domain types. Preserve the provider's identifier exactly; do not parse its structure, trim it, lowercase it, or treat it as a globally unique value.

Scope every folder, message, calendar, and event reference by `AccountId` in application operations and persistence keys. `AccountId` represents the connected provider account and must not be an email address or display name. A future second provider must add a provider namespace to account identity before its IDs enter the shared domain.

The Graph adapter will request `Prefer: IdType="ImmutableId"` for message and event operations that support it, including delta requests. Because the header applies only to the request that carries it, the adapter must include it on every relevant request. Mail folder IDs remain ordinary Graph IDs. If an immutable ID changes after an archive move or export and re-import, synchronization treats the old item as removed and the new item as added; it must not infer identity from `internetMessageId`.

Parse external IDs at the provider boundary and reject empty values. Keep Graph cursor links private to the adapter; they are not entity IDs and remain opaque sync state.

## Usage sketch

```text
messages = list_messages(account_id, folder_id)
mark_read(account_id, message_id)
```

Every mailbox operation is scoped by the account. The application treats the IDs as opaque values; only the Graph adapter knows the HTTP header and provider behavior.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Account-scoped opaque provider IDs | Uses the identifiers already returned by the first provider. Immutable Graph IDs preserve message and event references across ordinary folder moves. No local-to-remote mapping table is required. | Storage must preserve exact case. IDs can still change when an item moves to an archive mailbox or is exported and re-imported. A second provider needs an account namespace decision. |
| Local UUID for every domain entity plus a provider-ID mapping | Domain identity remains stable if a provider's identifier changes and can be shared across providers. The maintained `uuid` crate fits local UUID generation. | Adds a mapping store, reconciliation and migration rules, and transactional coupling between sync and mapping updates before the product has a second provider or a user-visible need for local identity. |
| Expose raw Graph ID strings and Graph resource types in application contracts | Minimal translation at the provider boundary. | Couples workflows to Graph formats, permits IDs of different kinds to be mixed, and violates the accepted provider boundary in ADR 0002. |

## Consequences

- Domain ID types stay provider-neutral at the API level, while retaining the provider-issued value unchanged.
- A folder's parent must belong to the same account and identify a different folder.
- Persistence uniqueness and lookup use the account ID plus the entity ID. SQLite comparisons must remain case-sensitive for provider IDs.
- The Graph provider owns the immutable-ID request header and translates raw Graph IDs into domain ID types.
- No local ID generator or UUID dependency is needed for the current identity model. Reconsider UUID-backed local identity before adding a second provider, cross-provider references, or app-owned objects that have no provider ID.
- Provider contract checks must cover case-preserving ID round trips, message moves, immutable-ID headers on delta requests, and the archive-move exception before storage is accepted.

## Sources

- [Microsoft Graph immutable Outlook identifiers](https://learn.microsoft.com/en-us/graph/outlook-immutable-id)
- [`uuid` 1.29.0 documentation](https://docs.rs/uuid/1.29.0/uuid/)
- [`uuid` source and manifest](https://github.com/uuid-rs/uuid/blob/main/Cargo.toml)
- [Exchange Online provider decision](0002-exchange-online-provider.md)
- [Local cache decision](0004-local-data-and-sync.md)
- [System architecture](../architecture/system.md)
