# ADR 0060: Support one active account in the first release

- Status: Accepted
- Date: 2026-11-10
- Owners: Einfach Mail maintainers

## Context

The first-release scope already describes one signed-in account, but the product brief still lists multiple-account support as an open question. M10 adds persistent navigation and an Accounts destination, so the interface needs a clear account model before it displays account-selection controls.

The current application workflow stores one optional active `AccountProfile` and rejects a second connection while that profile is active. The account router selects either Microsoft Graph or EWS for the connected account. The desktop workspace owns one `AccountSession`, its session-open errors, navigation, and child-view lifecycle. The account feature owns account operations, and Mail and Calendar receive focused session ports. Each session binds one `AccountId` and shares one per-account encrypted cache between the two workflows. Workspace sign-out stops child work, removes that cache, and only then removes the provider credential. Existing identity verification, cancellation, and retry rules protect this sequence.

This implementation does not provide an account collection or account switcher. Supporting simultaneous accounts would require coordinated changes to provider credential identity and enumeration, active-provider routing, desktop session ownership, synchronization lifecycle, and account removal.

## Decision

Support one active connected account at a time in the first release. The active account may use Microsoft Graph or Exchange EWS. A user may connect a different account after signing out of the current account through the existing cache-removal and credential-removal workflow.

Do not expose an account selector, add-account action while connected, combined inbox, parallel account sessions, or background synchronization for another account. The Accounts destination shows the current account identity and its supported account actions. Issue [#272](https://github.com/1fach/einfach-mail/issues/272) owns that UI behavior. Keep each account cache bound to its opaque `AccountId`; this decision does not change cache identity or persistence.

Expose the active provider kind beside the provider-independent `AccountProfile` through an application-owned `ConnectedAccount` value. Return the pair from restore, connect, and permission-review workflows so the Accounts view displays metadata from the same completed operation. Keep provider kind out of the domain profile.

Defer multiple simultaneously connected accounts and in-app account switching to a future product decision. That work must define an account inventory, per-provider credential identity, restore ordering, session ownership, cache lifecycle, and concurrent synchronization before adding a selector.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| One active account at a time | Matches the current account workflow and router, keeps one cache/session lifecycle, and still lets a user connect a different identity after sign-out. | Users must sign out before changing accounts; the local cache is removed through the existing sign-out flow. |
| Multiple active accounts with an in-app switcher | Matches the Wino account list and allows simultaneous mailboxes. | Requires account enumeration and coordinated changes to provider credentials, routing, sessions, sign-out, cache ownership, and synchronization. |
| Save multiple identities but activate one at a time | Could make switching faster without showing combined data. | Still requires a credential inventory, identity-safe restore, cache retention policy, and per-account session lifecycle; it adds most of the account-management complexity without a first-release user need. |

### Provider identity

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Put provider kind in `AccountProfile` | Every profile consumer can read the provider directly. | Makes a domain identity depend on a provider adapter and removes the profile's provider-independent contract. |
| Query provider kind separately from the workflow result | Leaves `AccountProfile` unchanged. | Splits one account result across operations and can observe provider state after it changes. |
| Return an application-owned `ConnectedAccount` | Keeps the domain profile neutral and returns provider metadata with the account operation that established it. | Updates the account workflow result types and their callers together. |

Use `ConnectedAccount`. The application workflow owns provider selection, and the desktop needs provider metadata to explain the active account. Pairing both values at that boundary keeps their identity and timing aligned.

## Consequences

- The first release has one active account identity and one active provider-backed session.
- Connecting a different identity requires successful sign-out first. Sign-out removes the active account's local cache before deleting its provider credential; failures preserve the existing retry path.
- Mail and Calendar keep using focused ports from the same account session and the same account-scoped cache.
- The native Accounts destination may show the active identity, but must not imply that users can switch or add another connected account.
- `ConnectedAccount` pairs the provider kind with `AccountProfile` in the application layer. Domain account identity remains provider-independent.
- Multi-account support requires a separate decision and an explicit migration and recovery design. This decision adds no schema or credential-store migration.

## Sources

- [Issue #268: Define one active account for the first release](https://github.com/1fach/einfach-mail/issues/268)
- [Issue #272: Put account setup and active-account details in the workspace](https://github.com/1fach/einfach-mail/issues/272)
- [ADR 0027: Serialize account removal with mail operations](0027-serialize-mail-account-removal-with-mail-operations.md)
- [ADR 0030: Order account cancellation against credential commits](0030-account-operation-cancellation-boundary.md)
- [ADR 0041: Bind saved sign-in to a verified cache identity](0041-bind-saved-sign-in-to-cache-identity.md)
- [ADR 0043: Use one per-account mail and calendar session](0043-per-account-mail-and-calendar-session.md)
- [ADR 0046: Use one account cache removal port](0046-account-cache-removal-port.md)
- [ADR 0059: Use a shared, adaptive mail and calendar workspace](0059-native-mail-and-calendar-workspace.md)
- `crates/application/src/account.rs`: one active profile and connection reservation
- `crates/providers/account-router/src/lib.rs`: one selected provider
- `apps/desktop/src/app/workspace.rs`: shared session, navigation, and child-view lifecycle owner
- `apps/desktop/src/features/account.rs`: account status, connection, permission review, and sign-out workflow
- `apps/desktop/src/features/account/state.rs`: one account status and provider metadata
- `crates/application/src/account/session.rs`: account-bound shared Mail and Calendar session
- `crates/providers/microsoft-graph/src/credential.rs` and `crates/persistence/src/exchange_ews_settings.rs`: provider-specific saved credentials and settings
