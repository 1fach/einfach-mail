# Microsoft Graph research

Research checked: 2026-10-31. Use official Microsoft documentation as the source of truth. API behavior can change; recheck these pages before implementing or releasing an integration.

## Findings

### Exchange Online

Microsoft recommends migrating Exchange Online EWS integrations to Microsoft Graph. Microsoft's current published timeline says phased global disablement begins in October 2026 and EWS is fully disabled in April 2027. Microsoft Graph does not support Exchange Server installed on premises. The project therefore targets Exchange Online through Graph and leaves on-premises Exchange out of the first release.

### Mail and calendar synchronization

- Graph message delta queries track added, changed, and deleted messages within a mail folder. A sync can return multiple pages. The client must continue through `@odata.nextLink` and save the final `@odata.deltaLink` for the next round.
- Graph event delta queries apply to a calendar view with a start and end time range. The delta link belongs to that view. Calendar delta behavior for the entire calendar collection is a different endpoint and must not be confused with event synchronization.
- Store provider links as opaque values. Do not parse or synthesize skip tokens or delta tokens.

### Calendar date-time and time zones

Graph's `dateTimeTimeZone` represents a local date and time together with a time-zone identifier. The identifier can be a Windows time-zone name or one of the additional zones supported by the calendar API; create and update methods may accept a narrower set. Keep the original identifier alongside the wall-clock value at the provider boundary. A fixed UTC offset cannot represent future daylight-saving changes. See the [dateTimeTimeZone resource](https://learn.microsoft.com/en-us/graph/api/resources/datetimetimezone?view=graph-rest-1.0) and the [Rust time-zone dependency review](../project/dependency-review.md#candidates) before implementing conversion or recurrence behavior.

### Outlook item identifiers

Graph IDs are case-sensitive. Standard Outlook item IDs can change when an item moves. For messages and events, the Graph adapter can request immutable IDs with `Prefer: IdType="ImmutableId"`; it must include that header on each request where immutable IDs are required, including delta calls. Folder IDs remain stable without immutable-ID support. An immutable ID can still change when an item moves to an archive mailbox or is exported and re-imported. The provider must treat that as removal plus addition rather than joining records by `internetMessageId`. See [ADR 0009](../decisions/0009-account-scoped-domain-identifiers.md) for the domain identity model.

### Throttling

- Graph returns HTTP 429 when a client exceeds a service limit. The response includes a `Retry-After` header with the suggested wait in seconds. Wait for that interval before retrying.
- If Graph omits `Retry-After`, use exponential backoff. Do not retry immediately.
- Do not retry an operation automatically unless sending it again is safe. Prove duplicate-request behavior before retrying a write.

### Permissions

Use delegated permissions and request only the scopes needed for the feature the user is connecting or reviewing. [ADR 0045](../decisions/0045-incremental-graph-permissions.md) records the policy and exact scope sets.

| Request | Delegated scopes | Trigger |
| --- | --- | --- |
| Initial connection | `User.Read`, `Mail.Read`, `Calendars.Read`, `offline_access` | The user connects an account. |
| Mail changes | `User.Read`, `Mail.ReadWrite`, `Calendars.Read`, `offline_access` | Microsoft denies a supported mail change, and the user chooses permission review. |
| Calendar changes | `User.Read`, `Mail.Read`, `Calendars.ReadWrite`, `offline_access` | Microsoft denies a supported event change, and the user chooses permission review. |
| Sending | `User.Read`, `Mail.Read`, `Calendars.Read`, `Mail.Send`, `offline_access` | Microsoft denies sending, and the user chooses permission review. |

`Mail.ReadWrite` permits creating, reading, updating, and deleting mail. It does not grant send access. `Calendars.ReadWrite` permits creating, reading, updating, and deleting calendar events. `Mail.Send` permits sending mail as the signed-in user without `Mail.ReadWrite`. Einfach Mail discloses each permission's full Microsoft capability before opening the system browser. A tenant's consent policy can still require administrator approval.

Microsoft grants an additional delegated permission alongside existing grants. Requesting a smaller scope set later does not revoke an earlier grant. Local sign-out also does not revoke consent at Microsoft.

Do not request application permissions for this signed-in desktop client. They can grant access without a signed-in user and require broader administrative consent.

Desktop sign-in, redirect registration, and Conditional Access constraints are covered in [Microsoft identity research](microsoft-identity.md).

## Upstream source snapshots

Snapshot pins reviewed on 2026-11-06:

| Snapshot | Upstream commit | License |
| --- | --- | --- |
| [Thunderbird desktop](../../repos/thunderbird/) | `177df5d51f7f9a3528514fb72d44fe67ed1b1fcb` | No root license file or single SPDX license is identified. Preserve nested notices and license files. |
| [Wino Mail](../../repos/wino-mail/) | `c569dff5a4f97865b5258dc7960ec1d5a5fb1bbd` | GPL-3.0 |
| [Microsoft Graph Go SDK](../../repos/msgraph-sdk-go/) | `11b148f27ef6db3fdba4a5a26b948d191c6a2eb6` | MIT |
| [io-msgraph Rust client](../../repos/io-msgraph/) | `20616935dd4cb67547493ae77db07bc4d3948dbd` | MIT OR Apache-2.0 |

Use these trees only for source inspection. Do not add them as application dependencies. Microsoft documentation and accepted ADRs remain authoritative. Einfach Mail has copied no implementation code from these projects.

## Sources

- [Migrate Exchange Web Services apps to Microsoft Graph](https://learn.microsoft.com/en-us/graph/migrate-exchange-web-services-overview)
- [Deprecation of Exchange Web Services in Exchange Online](https://learn.microsoft.com/en-us/exchange/clients-and-mobile-in-exchange-online/deprecation-of-ews-exchange-online)
- [Microsoft Graph message delta](https://learn.microsoft.com/en-us/graph/api/message-delta?view=graph-rest-1.0)
- [Microsoft Graph immutable Outlook identifiers](https://learn.microsoft.com/en-us/graph/outlook-immutable-id)
- [Get incremental changes to events in a calendar view](https://learn.microsoft.com/en-us/graph/delta-query-events?tabs=http)
- [Microsoft Graph dateTimeTimeZone resource and supported zones](https://learn.microsoft.com/en-us/graph/api/resources/datetimetimezone?view=graph-rest-1.0)
- [Microsoft Graph throttling guidance](https://learn.microsoft.com/en-us/graph/throttling)
- [Microsoft Graph permissions reference](https://learn.microsoft.com/en-us/graph/permissions-reference?view=graph-rest-1.0)
- [Microsoft Entra guide to requesting permissions and consent](https://learn.microsoft.com/en-us/entra/identity-platform/consent-types-developer)
