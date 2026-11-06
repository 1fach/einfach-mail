# ADR 0015: Add idempotent message read-state updates

- Status: Accepted
- Date: 2026-11-01

## Context

M2 keeps the mail provider read-only under [ADR 0011](0011-initial-mail-sync-scope.md). M4 begins with one explicit action: setting a cached message to read or unread. The Graph transport in [ADR 0006](0006-graph-http-transport.md) currently sends GET requests and does not automatically retry writes.

Microsoft Graph v1.0 updates a message through `PATCH /me/messages/{id}`. It accepts `isRead` in the request body, requires `Content-Type: application/json`, and documents a `200 OK` response with the updated message. The least-privileged delegated permission listed for this operation is `Mail.ReadWrite`. Microsoft describes that permission as allowing the app to create, read, update, and delete mail for the signed-in user; it does not grant sending mail. The app currently requests `Mail.Read`, so a connected account needs an explicit permission update that does not discard its local cache.

No dedicated Microsoft 365 test tenant is available. Local provider and workflow behavior can be covered with synthetic Graph responses, but consent and server writes need separate tenant evidence.

## Decision

- Implement only an explicit set-read-state operation for a message already cached in the account and folder supplied by the caller. Accept a desired `ReadState`; do not expose a blind toggle or queue the action for offline replay.
- Send one Graph v1.0 PATCH to `/me/messages/{id}` with a JSON body containing only `isRead`. Parse the returned `isRead` and require it to match the requested state before reporting server confirmation.
- Do not automatically retry PATCH requests. After an uncertain result, require the affected folder to complete a delta round before exposing another message write.
- Treat redirects, server errors, request timeouts, transport failures, and worker loss after dispatch as unknown outcomes. Treat authorization and throttling responses as explicit failures; do not retry a throttled PATCH.
- Reserve the folder operation while the write is in progress, so a delta sync cannot commit an older read-state value over the action. Commit an account-and-folder recovery marker to SQLCipher before dispatching the PATCH. Persist the confirmed state and clear the marker in one transaction; leave it in place after an uncertain result or failed cache write. ADR 0019 shares this marker with follow-up flag updates.
- If Graph confirms the state but the local cache update fails, return an outcome that distinguishes remote confirmation from local persistence. Update the open view to reflect the confirmed state and ask the user to sync the folder.
- Keep message-write controls unavailable in a new mail view until its initial account sync succeeds. If sync fails, keep the controls unavailable and explain that another successful sync is required. This preserves the reconciliation requirement after an uncertain write when the user closes and reopens Mail.
- Add an explicit permission-update flow for an already connected account. It requests the current delegated scopes with `Mail.ReadWrite` in place of `Mail.Read`, checks that the returned profile has the existing account ID, and only then replaces the saved refresh token and active access token. Cancellation, denial, identity mismatch, or storage failure leaves the existing session and encrypted cache in place.
- Explain the full `Mail.ReadWrite` capability before opening the browser. Do not request `Mail.Send` or add create, delete, move, archive, flag, or send actions in this change. The authorization flow remains delegated for the signed-in user.
- Put the read-state action in the native message detail header. Show pending, confirmed, denied, offline, and failure feedback. Give a denied action a direct route back to Accounts, where the user can review permissions.
- Use synthetic provider, workflow, persistence, and headless GPUI coverage. Keep live consent and write verification in [issue 24](https://github.com/1fach/einfach-mail/issues/24); do not use a personal mailbox or claim tenant behavior from fixtures.

This decision supersedes the GET-only transport restriction in ADR 0006 only for the message `isRead` PATCH. It supersedes the `Mail.ReadWrite` prohibitions in ADRs 0003 and 0013 and the no-write permission rule in ADR 0014 only for this delegated read-state action. Initial sign-in remains read-only, calendar access remains read-only, and the app does not request `Mail.Send`.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Send PATCH, then commit cache state after a matching Graph response | Keeps visible and cached values tied to server confirmation; repeated explicit requests converge. | A response can be lost after Graph applies the update; the cache can be stale until a retry or sync. |
| Update the cache optimistically and roll it back after failure | Makes the view respond immediately. | A failed or ambiguous request can leave local state inconsistent and rollback can overwrite a later sync. |
| Queue actions in an offline outbox | Lets users act without a connection. | Requires conflict ordering, durable operation identity, retry, and reconciliation rules that the first action does not need. |
| Require sign-out and a new connection to add `Mail.ReadWrite` | Reuses the current initial sign-in flow. | Sign-out deletes the encrypted cache and makes permission renewal unnecessarily destructive. |
| Mark a message read automatically when the user opens it | Removes a control from the message view. | Opening a message would create an implicit remote write and would not give users a direct unread action. |

## Consequences

- The OAuth scope set replaces delegated `Mail.Read` with `Mail.ReadWrite` while retaining `User.Read`, `Calendars.Read`, and `offline_access`. Existing accounts can request the updated grant without clearing their cache.
- The broader permission can create and delete mail even though this feature only changes read state. The account UI must disclose that capability accurately; the app still has no send permission.
- A failed or ambiguous PATCH leaves cached state unchanged. Its durable marker blocks read-state and follow-up flag writes in the affected folder until a complete delta round commits.
- A failed initial account sync keeps message-write controls unavailable until a later sync succeeds. Cached messages remain available for reading.
- An account mismatch during permission renewal cannot replace the saved credential or active account.
- [Issue 23](https://github.com/1fach/einfach-mail/issues/23) tracks the local Linux implementation. [Issue 24](https://github.com/1fach/einfach-mail/issues/24) tracks test-tenant consent and Graph verification and remains open until that evidence exists.

## Sources

- [Microsoft Graph update message v1.0](https://learn.microsoft.com/en-us/graph/api/message-update?view=graph-rest-1.0)
- [Microsoft Graph permissions reference](https://learn.microsoft.com/en-us/graph/permissions-reference?view=graph-rest-1.0#mailreadwrite)
- [Microsoft identity platform incremental consent](https://learn.microsoft.com/en-us/entra/identity-platform/consent-types-developer)
- [ADR 0006: Graph HTTP transport](0006-graph-http-transport.md)
- [ADR 0011: Initial mail sync scope](0011-initial-mail-sync-scope.md)
- [Issue 23: Mark cached messages read or unread](https://github.com/1fach/einfach-mail/issues/23)
- [Issue 24: Verify message read-state updates in a Microsoft 365 test tenant](https://github.com/1fach/einfach-mail/issues/24)
