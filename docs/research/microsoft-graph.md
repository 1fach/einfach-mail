# Microsoft Graph research

Research checked: 2026-10-30. Use official Microsoft documentation as the source of truth. API behavior can change; recheck these pages before implementing or releasing an integration.

## Findings

### Exchange Online

Microsoft recommends migrating Exchange Online EWS integrations to Microsoft Graph. Microsoft's current published timeline says phased global disablement begins in October 2026 and EWS is fully disabled in April 2027. Microsoft Graph does not support Exchange Server installed on premises. The project therefore targets Exchange Online through Graph and leaves on-premises Exchange out of the first release.

### Mail and calendar synchronization

- Graph message delta queries track added, changed, and deleted messages within a mail folder. A sync can return multiple pages. The client must continue through `@odata.nextLink` and save the final `@odata.deltaLink` for the next round.
- Graph event delta queries apply to a calendar view with a start and end time range. The delta link belongs to that view. Calendar delta behavior for the entire calendar collection is a different endpoint and must not be confused with event synchronization.
- Store provider links as opaque values. Do not parse or synthesize skip tokens or delta tokens.

### Permissions

Use delegated permissions and ask only for scopes needed by the enabled workflow. A possible staged plan is `Mail.Read` and `Calendars.Read` for read-only access, then `Mail.ReadWrite`, `Mail.Send`, and `Calendars.ReadWrite` when the user enables the corresponding write features. `Mail.Send` is a separate scope and does not require `Mail.ReadWrite` to send a message. Confirm exact scope requirements against every endpoint before implementation.

Do not request application permissions for this signed-in desktop client. They can grant access without a signed-in user and require broader administrative consent.

### Desktop authentication

Microsoft recommends authorization code with PKCE and OpenID Connect for desktop applications. The desktop configuration guide specifies `http://localhost` for a system-browser redirect and identifies the app as a public client. Microsoft recommends a Microsoft-built authentication library.

The Microsoft desktop library table does not list Rust. This is a design risk, not evidence that a particular community crate is supported by Microsoft. The project requires an authentication spike for Windows sign-in, MFA, Conditional Access, token refresh, and OS credential storage before account access ships.

## Sources

- [Migrate Exchange Web Services apps to Microsoft Graph](https://learn.microsoft.com/en-us/graph/migrate-exchange-web-services-overview)
- [Deprecation of Exchange Web Services in Exchange Online](https://learn.microsoft.com/en-us/exchange/clients-and-mobile-in-exchange-online/deprecation-of-ews-exchange-online)
- [Microsoft Graph message delta](https://learn.microsoft.com/en-us/graph/api/message-delta?view=graph-rest-1.0)
- [Get incremental changes to events in a calendar view](https://learn.microsoft.com/en-us/graph/delta-query-events?tabs=http)
- [Microsoft Graph permissions reference](https://learn.microsoft.com/en-us/graph/permissions-reference?view=graph-rest-1.0)
- [Microsoft identity platform authorization code flow](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow)
- [Configure desktop apps that call web APIs](https://learn.microsoft.com/en-us/entra/identity-platform/scenario-desktop-app-configuration)
- [Cargo workspaces](https://doc.rust-lang.org/cargo/reference/workspaces.html)

No external mail-client repository has been copied into this project. Provider behavior should come from Microsoft API documentation and sanitized contract fixtures.
