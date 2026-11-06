# ADR 0013: Use the Graph profile for initial account identity

- Status: Accepted for Linux implementation; live Entra verification remains open
- Date: 2026-11-01
- Owners: Project maintainer

## Context

Issue #5 asks the Linux app to connect one Microsoft 365 work or school account and show the connected identity. The original proposal used the OpenID Connect `openid` and `profile` scopes. That path needs correct ID-token issuer validation for Microsoft's tenant-independent `organizations` authority. The selected Rust authorization library validates a configured issuer exactly; no reviewed Rust MSAL library provides Microsoft's multi-tenant validation behavior.

Wino Mail's shared authenticator calls Microsoft Graph `/me` after sign-in and selects `mail` and `userPrincipalName`. Its source explains that the sign-in UPN can differ from the mailbox's primary address. Microsoft's Graph documentation lists delegated `User.Read` as the least-privileged permission for `/me`. The app needs the mailbox identity to label its connected account. M2 also fetches message bodies only when users open a message. Microsoft says `Mail.ReadBasic` excludes bodies, so delegated `Mail.Read` is the narrowest permission that covers the selected read-only mail workflow.

## Decision

Use Microsoft Graph `GET /me?$select=id,displayName,mail,userPrincipalName` for the first connected-account identity. Request delegated `User.Read`, `Mail.Read`, and `offline_access` through the `organizations` authority. Explain that `Mail.Read` is read-only and that the app fetches a message body only after the user opens it. Do not request `Mail.ReadWrite`, send permission, or application permissions.

Do not request `openid` or `profile`, consume an ID token, or add a custom multi-tenant OIDC issuer validator for this workflow. The provider parses the Graph response at its boundary and returns a provider-independent account identity. Use Graph `id` as the opaque account identifier. Use `displayName` for the label. Prefer `mail` for the mailbox address and retain `userPrincipalName` separately because they can differ. Treat `mail` as optional.

Keep access and refresh tokens inside the Microsoft provider. Do not expose Graph response types to application workflows or GPUI views. Do not request `User.Read.All`, `Mail.ReadWrite`, send permission, or application permissions.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Use validated OIDC claims | Needs no Graph profile request or Graph permission for account display. | The Rust libraries reviewed do not provide the required Microsoft multi-tenant issuer and signing-key issuer checks without unresolved verifier or crypto concerns. The display claims also do not guarantee the mailbox's primary SMTP address. |
| Call Graph `/me` with `User.Read` | Uses Graph's documented least-privileged permission for the endpoint and returns the mailbox profile fields needed by a mail client. Wino uses this pattern when the sign-in name differs from the mailbox address. | Adds one Graph request and profile access during account connection. The tenant's consent behavior remains unverified. |
| Use `Mail.ReadBasic` for folder and message metadata | Limits access to basic message properties while supporting the current folder and message-list endpoints. | It excludes message bodies and cannot support ADR 0011's open-body behavior without another consent grant. |
| Use delegated `Mail.Read` for the read-only M2 workflow | Supports metadata reads and the selected fetch-on-open message body path without mail writes or send access. | Grants read access to the signed-in user's mail. The provider must fetch bodies only after an explicit open action and store them only in the approved encrypted cache. |

## Consequences

- The first account-connection consent explains `User.Read`, `Mail.Read`, and `offline_access` before opening the system browser.
- The Graph adapter validates the profile response shape and maps it into domain values. It never stores or returns the access token with the profile.
- A missing `mail` value does not invalidate the connected identity. The UI can show the user principal name with a clear label.
- Use synthetic profile responses until a dedicated test tenant is available. Do not claim live consent or account behavior until issue #3 passes.
- Revisit this decision if live tenant evidence shows Graph `/me` cannot provide the required mailbox profile or consent is unacceptable.

## Sources

- [Microsoft Graph `GET /me` and permissions](https://learn.microsoft.com/en-us/graph/api/user-get?view=graph-rest-1.0)
- [Microsoft Graph get message and permissions](https://learn.microsoft.com/en-us/graph/api/message-get?view=graph-rest-1.0)
- [Microsoft Graph permission reference for `Mail.Read` and `Mail.ReadBasic`](https://learn.microsoft.com/en-us/graph/permissions-reference?view=graph-rest-1.0#mailread)
- [Microsoft identity platform scopes and `offline_access`](https://learn.microsoft.com/en-us/entra/identity-platform/scopes-oidc)
- [Wino Mail profile lookup at the reviewed snapshot](https://github.com/bkaankose/Wino-Mail/blob/517b9b9e42540ed76c732e926f6e05b91d8c322c/src/Wino.Authentication/OutlookAuthenticator.cs)
- [Issue #3: Prove delegated Entra desktop sign-in](https://github.com/1fach/einfach-mail/issues/3)
- [Issue #5: Connect an account and show its identity](https://github.com/1fach/einfach-mail/issues/5)
- [ADR 0008: Keep identity tokens inside the Microsoft provider](0008-identity-token-boundary.md)
