# ADR 0045: Request Graph permissions incrementally

- Status: Accepted for Linux implementation; tenant verification remains open
- Date: 2026-11-06
- Owners: Einfach Mail maintainers

## Context

The first account connection supports profile lookup, read-only mail, and a read-only primary calendar. M4 adds mail changes, calendar changes, and sending. Those actions need broader delegated Graph permissions than the first connection.

Microsoft Entra supports dynamic delegated consent. An app can request a minimum scope set first, then request feature scopes as the user uses those features. A grant adds to existing consent. A later request with fewer scopes does not revoke an earlier grant. Tenant policy can still require administrator approval.

The Microsoft provider has typed permission requests for the initial connection, mail changes, calendar changes, and sending. It sends each request through the same-account authorization-code flow and checks the Graph profile before replacing the saved refresh token. No dedicated Microsoft 365 test tenant or public app registration is available.

## Decision

Use exactly these delegated scope sets. Keep mail-change, calendar-change, and send permissions out of initial connection.

| Request | Exact delegated scope set | Trigger |
| --- | --- | --- |
| Initial connection | `User.Read`, `Mail.Read`, `Calendars.Read`, `offline_access` | The user connects an account. |
| Mail changes | `User.Read`, `Mail.ReadWrite`, `Calendars.Read`, `offline_access` | Microsoft denies a supported mail change, and the user reviews its full access. |
| Calendar changes | `User.Read`, `Mail.Read`, `Calendars.ReadWrite`, `offline_access` | Microsoft denies a supported event change, and the user reviews its full access. |
| Sending | `User.Read`, `Mail.Read`, `Calendars.Read`, `Mail.Send`, `offline_access` | Microsoft denies sending, and the user reviews its full access. |

`Mail.ReadWrite` replaces `Mail.Read` in the mail-change request. `Calendars.ReadWrite` replaces `Calendars.Read` in the calendar-change request. The send request adds `Mail.Send` to the initial scope set. Keep the other baseline scopes so the provider can verify the signed-in identity and retain the active views' read access.

Before opening the system browser, explain the full Microsoft capability of the requested permission and the narrower actions Einfach Mail currently supports. Permission approval does not retry or submit the failed operation. Preserve the current account session and cache after cancellation, consent denial, identity mismatch, or credential failure.

Do not request application permissions. Do not claim that requesting fewer scopes or signing out revokes an existing grant. `Mail.ReadWrite` does not include send permission. `Mail.Send` permits sending mail as the signed-in user without `Mail.ReadWrite`.

## Alternatives

| Option | Benefit | Cost or risk |
| --- | --- | --- |
| Request every implemented scope on initial connection | Uses one authorization request. | Requests broad mail and calendar mutation access before the user uses those features. It contradicts the read-only initial connection. |
| Request only the new scope after denial | Sends a shorter later scope list. | Omits baseline scopes used to verify the account and retain mail or calendar read access. |
| Request feature-specific scopes after a user-reviewed denial | Keeps initial consent read-only and ties each additional permission to a user action. | Adds a system-browser review and a later retry action. Tenant consent policy can still require administrator approval. |

## Consequences

- The provider owns exact scope sets for the initial connection and each feature request. Synthetic provider tests assert those sets and exclude unrelated write permissions.
- The account view explains the full permission grant before the user continues to Microsoft.
- A cancelled, denied, mismatched-account, or failed authorization leaves the current session and cache available.
- Existing grants remain additive. The app cannot remove Microsoft's consent grant through local sign-out or a later request with fewer scopes.
- Issue #3 remains open for live sign-in and consent evidence. Keep M4 tenant proof in the feature-specific issues: #24, #26, #28, #30, #32, #34, #36, #38, #40, and #42. Synthetic tests do not verify tenant consent or live Graph behavior.

## Sources

- [Microsoft Entra guide to requesting permissions and consent](https://learn.microsoft.com/en-us/entra/identity-platform/consent-types-developer)
- [Microsoft Graph permissions reference](https://learn.microsoft.com/en-us/graph/permissions-reference?view=graph-rest-1.0)
- [Microsoft Graph update message v1.0](https://learn.microsoft.com/en-us/graph/api/message-update?view=graph-rest-1.0)
- [Microsoft Graph create event v1.0](https://learn.microsoft.com/en-us/graph/api/calendar-post-events?view=graph-rest-1.0)
- [Microsoft Graph sendMail v1.0](https://learn.microsoft.com/en-us/graph/api/user-sendmail?view=graph-rest-1.0)
- [ADR 0003: Desktop authentication](0003-desktop-authentication.md)
- [ADR 0015: Message read-state updates](0015-idempotent-message-read-state-update.md)
- [ADR 0016: Primary calendar event creation](0016-idempotent-primary-calendar-event-creation.md)
- [ADR 0017: Plain-text mail sending](0017-safe-plain-text-mail-sending.md)
- [ADR 0018: Native plain-text reply](0018-native-plain-text-message-reply.md)
- [Issue #3: Prove delegated Entra desktop sign-in](https://github.com/1fach/einfach-mail/issues/3)
