# ADR 0041: Bind saved sign-in to a verified cache identity

- Status: Accepted for the Linux M1 account flow
- Date: 2026-11-05
- Owners: Einfach Mail maintainers

## Context

Issue #48 addresses a gap between the Microsoft refresh credential in Linux Secret Service and the account-scoped SQLCipher cache. If restore fails before Graph returns `/me`, the application cannot tell which cache belongs to the credential. Removing the credential first can leave its cache behind. Removing another cache can delete the wrong account's data.

Older token-only and version 1 credentials do not prove that Graph verified their saved account ID. A refresh token can also rotate before the provider requests `/me`, so the rotated token and the prior expected ID must survive a restart together.

## Decision

- Keep the refresh token and its identity state in one versioned Secret Service record owned by the Microsoft Graph provider.
- Read token-only records as unverified with no expected account ID. Read version 1 account IDs as unverified expected IDs. Read version 2 account IDs as verified IDs and version 2 `expected_account_id` values as pending checks.
- During restore, retain the expected ID and persist the rotated token as pending before requesting Graph `/me`. Reject a profile that differs from the expected ID. Save a verified ID only after Graph returns a matching profile.
- Return a saved account ID to the application only when the provider has marked it verified. Keep access and refresh tokens inside the provider.
- During routine access-token refresh, require the verified saved ID to match the active provider session. Preserve that verified ID when saving a rotated token.
- Before sign-out, require a verified saved ID and remove its cache through the existing account-removal guard. Delete the Secret Service credential only after cache removal succeeds. Keep unverified credentials and reject replacement sign-in while their identity remains unverified.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Store identity state beside the refresh token | A single Secret Service read or write keeps token rotation and identity state together. The provider can verify the cache owner before removal. | Requires backward-compatible parsing and an explicit pending state. |
| Store the cache account ID in application or database state | Lets the UI find an account without reading the provider credential. | Duplicates identity state and can bind the cache to a different token after credential replacement. |
| Scan or remove every local cache after restore fails | Could find old caches without a verified credential ID. | Cannot prove which cache belongs to this credential and may remove another account's data. |

## Consequences

- Token-only credentials remain readable, but the provider cannot expose their identity until Graph verifies `/me`.
- A failed or mismatched restore leaves a pending credential that can be checked again after restart. A mismatch blocks account replacement and credential deletion until the saved account can be verified.
- A cache-removal error leaves the verified credential in Secret Service, so the user can retry removal.
- Synthetic tests cover credential formats, rotation, restart, mismatch, replacement blocking, and cache-removal ordering. They do not verify Microsoft Entra or a desktop Secret Service session.
- Live Entra sign-in remains under issue #3. No tenant or real mailbox data is required for this local boundary.

## Sources

- [GitHub issue #48: Bind saved sign-in to its encrypted cache identity](https://github.com/1fach/einfach-mail/issues/48)
- [ADR 0008: Keep identity tokens inside the Microsoft provider](0008-identity-token-boundary.md)
- [ADR 0027: Serialize mail account removal with mail operations](0027-serialize-mail-account-removal-with-mail-operations.md)
- [ADR 0029: Keep cache removal retryable after a sidecar failure](0029-retry-cache-removal-after-sidecar-failure.md)
- [ADR 0030: Order account cancellation against credential commits](0030-account-operation-cancellation-boundary.md)
- `crates/providers/microsoft-graph/src/credential.rs` and `crates/providers/microsoft-graph/src/provider.rs`
