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

Use delegated permissions and ask only for scopes needed by the enabled workflow. A possible staged plan is `Mail.Read` and `Calendars.Read` for read-only access, then `Mail.ReadWrite`, `Mail.Send`, and `Calendars.ReadWrite` when the user enables the corresponding write features. `Mail.Send` is a separate scope and does not require `Mail.ReadWrite` to send a message. Confirm exact scope requirements against every endpoint before implementation.

Do not request application permissions for this signed-in desktop client. They can grant access without a signed-in user and require broader administrative consent.

Desktop sign-in, redirect registration, and Conditional Access constraints are covered in [Microsoft identity research](microsoft-identity.md).

## Sources

- [Migrate Exchange Web Services apps to Microsoft Graph](https://learn.microsoft.com/en-us/graph/migrate-exchange-web-services-overview)
- [Deprecation of Exchange Web Services in Exchange Online](https://learn.microsoft.com/en-us/exchange/clients-and-mobile-in-exchange-online/deprecation-of-ews-exchange-online)
- [Microsoft Graph message delta](https://learn.microsoft.com/en-us/graph/api/message-delta?view=graph-rest-1.0)
- [Microsoft Graph immutable Outlook identifiers](https://learn.microsoft.com/en-us/graph/outlook-immutable-id)
- [Get incremental changes to events in a calendar view](https://learn.microsoft.com/en-us/graph/delta-query-events?tabs=http)
- [Microsoft Graph dateTimeTimeZone resource and supported zones](https://learn.microsoft.com/en-us/graph/api/resources/datetimetimezone?view=graph-rest-1.0)
- [Microsoft Graph throttling guidance](https://learn.microsoft.com/en-us/graph/throttling)
- [Microsoft Graph permissions reference](https://learn.microsoft.com/en-us/graph/permissions-reference?view=graph-rest-1.0)

No external mail-client repository has been copied into this project. Provider behavior should come from Microsoft API documentation and sanitized contract fixtures.
