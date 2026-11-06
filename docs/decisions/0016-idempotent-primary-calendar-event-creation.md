# ADR 0016: Create one-off timed events in the primary calendar

- Status: Accepted
- Date: 2026-11-01

## Context

M3 reads a fixed week from the signed-in user's primary calendar through Graph's `calendarView/delta` endpoint and stores the result in the encrypted per-account cache. M4 now needs one explicit calendar write. The account workflow can renew delegated consent without signing out, and the Graph transport does not automatically retry writes. There is no dedicated Microsoft 365 test tenant, so local acceptance must use synthetic provider responses and must not claim live consent or event creation.

Microsoft Graph v1.0 creates an event in the primary calendar through `POST /me/calendar/events`. The least-privileged delegated permission for a user's calendar is `Calendars.ReadWrite`; this permission allows more calendar access than the single action this version exposes. A created event can include `transactionId`, which Graph documents for avoiding redundant POST operations after a client retry. The Graph `dateTimeTimeZone` type accepts Windows zones and additional calendar zones, but Microsoft warns that create and update methods may not support every time-zone ID.

ADR 0012 requires a separate source local date-time and zone type for event creation and an explicit policy for unsupported zones and DST gaps or overlaps. The existing local calendar view uses Jiff at the Linux presentation boundary, while the provider and domain represent event times as UTC instants. The GPUI Kit date-and-time picker exposes Chrono's naive date-time value; this is a UI component boundary, not a domain or provider time model.

## Decision

- Implement one explicitly submitted, timed, non-recurring appointment in the signed-in user's primary calendar. Provide a native GPUI Kit sheet with a subject and start/end date-and-time fields. Limit both endpoints to the displayed week. Do not expose all-day events, recurrence, attendees, meeting invitations, online meetings, body, location, event updates, or event deletion.
- Keep the source schedule as a domain value containing local start and end date-times and an opaque IANA zone identifier, alongside the resolved UTC instants. The domain uses `time` value types and does not depend on GPUI, Chrono, Jiff, Graph, or persistence. Convert the GPUI Kit Chrono picker value to the domain local date-time at the desktop boundary.
- Reuse the locked Jiff 0.2.38 and the Linux host zoneinfo database to resolve local times. For each endpoint, require `TimeZone::to_ambiguous_zoned(...).unambiguous()` to succeed. Reject both nonexistent times in a DST gap and repeated times in a DST fold; do not guess which instant the user intended. When the system zone has no IANA name, clearly label the composer as UTC and interpret input as UTC. Never substitute an undocumented or inferred zone.
- Send the resolved instants to Graph as UTC `dateTimeTimeZone` values with `timeZone: "UTC"`. This avoids relying on Graph event creation support for arbitrary Linux IANA zones. Keep source local values and the source zone in the encrypted pending draft so recovery preserves the user's original input.
- Request only delegated `Calendars.ReadWrite` as the new scope, retaining the existing delegated profile, mail, calendar-read, and refresh scopes. Use the existing same-account reauthorization path and preserve the active account and cache on denial, cancellation, identity mismatch, or credential-store failure. Before opening the system browser, disclose that Microsoft grants create/read/update/delete calendar capability while this version reads calendar events and creates one-off timed appointments. Do not request application permissions or mail-send permission.
- Send one Graph v1.0 `POST /me/calendar/events` with a UUID `transactionId`, the subject, UTC start, and UTC end. Omit attendees, body, location, recurrence, and online-meeting fields. Request immutable Outlook IDs so the returned event can be cached alongside delta records. Do not automatically retry POST, including on throttling.
- Before the POST, store the account-scoped operation ID and exact draft in the encrypted per-account cache. Allow a retry to replay only that same draft with the same `transactionId`; keep it recoverable after a lost response, app restart, access denial, malformed response, or local cache failure. There is at most one unresolved event-creation operation per account. A new creation is not staged until the previous operation is confirmed and removed.
- Classify authorization failures separately, treat only definitive non-throttling client rejections as rejected, and keep throttled, timed-out, conflicting, server, redirect, malformed, and transport outcomes unresolved. Never retry the POST automatically.
- Persist the provider's definitive rejection before clearing its operation marker. If either local step fails, keep the saved operation; recovery retries only the local rejection record and cleanup, without another Graph POST. The account-removal guard applies to unresolved operations, while a recorded rejection can be removed safely.
- Treat only a validated Graph `201 Created` response containing the expected subject and a complete, non-empty UTC timed range as confirmation. In one database transaction, insert the confirmed event into the exact week cache and remove the matching pending operation. If the Graph response is lost or the confirmed event cannot be cached, retain the pending operation and show a retry or sync action. The calendar view and delta sync cannot concurrently reserve the same week while an event creation is in flight.
- Keep tenant-only verification separate in issue #26. Local tests use synthetic HTTP responses and timestamps. They do not establish that Entra consent or a real Microsoft 365 mailbox behaves as expected.

This decision supersedes ADR 0014's read-only-only restriction for the single create action described here. It adds `Calendars.ReadWrite` to the account consent set and supersedes the calendar scope copy in ADR 0015 only by adding that permission and its corresponding disclosure; it does not change the mail action.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Persist one pending operation and replay it with Graph `transactionId` | Recovers the exact user intent after a timeout or restart and gives retries a stable server-side deduplication key. Confirmed event persistence and pending removal can be atomic. | Adds a small encrypted pending-operation table and prevents another create for that account until the uncertain operation is resolved. |
| Keep the operation ID only in the open composer | Avoids a persistence migration. | A restart or sheet dismissal loses the key after Graph may have created the event, so a user retry can create a duplicate. |
| Update the cache optimistically before Graph confirmation | Makes the agenda change immediately. | A denied or uncertain request can create a false local event; rollback can race with sync. |
| Retry a POST automatically with a new request | Simplifies transport control flow. | A timeout after server creation can produce duplicate events. Graph's `transactionId` exists to let the client safely identify retries, so this client must keep and reuse it. |
| Send arbitrary Linux IANA values to Graph | Preserves a named input zone in the remote event. | Microsoft's event documentation warns that create methods do not support every additional zone. UTC instants are unambiguous and already match the app's M3 response contract. |
| Resolve event time with `chrono-tz` or a bundled timezone database | Can resolve IANA schedules without relying on host zoneinfo. | Adds a second date-time model and bundled timezone database when the Linux app already uses Jiff with the system zoneinfo database. That database would also need its own update policy. |
| Use the `time` crate alone for local-zone resolution | Reuses a workspace crate for date-time values and parsing. | It does not provide the Linux system IANA database or gap/fold resolution required by ADR 0012. |

## Consequences

- The domain gains separate source schedule, zone ID, creation ID, and event draft values. The cached occurrence remains an account-and-view-scoped UTC event.
- The Linux desktop uses GPUI Kit's native date-and-time picker and converts its Chrono boundary values to the provider-independent domain model. Jiff remains the only time-zone resolver; Chrono is not used for zone conversion, Graph serialization, or domain values.
- The Graph adapter gains one non-retried JSON POST operation and validates the full created event response. It sends a stable transaction ID and requests immutable event IDs.
- The encrypted cache gains an account-scoped pending creation record with unresolved and rejected states, plus migrations that preserve legacy operations for same-ID recovery. A transaction stores a confirmed event while clearing the matching pending record. Account removal is blocked while an unresolved creation exists, preserving the cache, encryption key, and draft for same-operation recovery; recorded rejections can be cleared locally.
- An ambiguous outcome remains visible and recoverable. The app does not silently discard the pending draft or issue a second operation with a different transaction ID.
- `Calendars.ReadWrite` is broader than this action. The account screen and permission-renewal control must disclose its full Microsoft capability and current app use accurately.
- Local synthetic checks establish serialization, provider mapping, workflow coordination, encrypted persistence, and headless UI state. Issue #26 remains open until a dedicated test tenant verifies delegated consent, a real create response, sync visibility, and the same-transaction retry behavior.

## Sources

- [Microsoft Graph create event v1.0](https://learn.microsoft.com/en-us/graph/api/calendar-post-events?view=graph-rest-1.0)
- [Microsoft Graph event resource and `transactionId`](https://learn.microsoft.com/en-us/graph/api/resources/event?view=graph-rest-1.0)
- [Microsoft Graph `dateTimeTimeZone` resource](https://learn.microsoft.com/en-us/graph/api/resources/datetimetimezone?view=graph-rest-1.0)
- [Jiff time-zone disambiguation](https://docs.rs/jiff/0.2.38/jiff/tz/struct.TimeZone.html)
- [Rust dependency review: calendar occurrence times and Linux display](../project/dependency-review.md#candidates)
- [ADR 0006: Graph HTTP transport](0006-graph-http-transport.md)
- [ADR 0012: Graph calendar occurrence times](0012-graph-calendar-occurrence-times.md)
- [ADR 0014: Linux read-only primary calendar](0014-linux-read-only-calendar-workflow.md)
- [ADR 0015: Message read-state updates](0015-idempotent-message-read-state-update.md)
- [Issue #25: Create one-off timed events](https://github.com/1fach/einfach-mail/issues/25)
- [Issue #26: Verify calendar event creation in a test tenant](https://github.com/1fach/einfach-mail/issues/26)
