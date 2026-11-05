# ADR 0026: Delete standalone calendar appointments

- Status: Accepted
- Date: 2026-11-04

## Context

The calendar editor supports only timed, standalone primary-calendar appointments with no attendees or online meeting. Users need to remove the same supported appointments in Einfach Mail. Deleting an organizer's meeting can send cancellation messages, so the final provider check must reject missing or contradictory event metadata.

Microsoft Graph v1.0 deletes an event with `DELETE /me/events/{id}` and returns `204 No Content` on success. The Microsoft-maintained v1.0 OpenAPI specification declares `If-Match` as an optional request header for event deletion. Microsoft's event GET example returns a weak `@odata.etag`. OData 4.01 section 8.2.4 specifies weak comparison for services that send weak entity-state ETags. It also permits `If-Match` on DELETE. The adapter preserves the ETag unchanged. The Learn page does not list the header. Issue #42 must verify Exchange Online behavior.

## Decision

- Offer Delete only for the same checked event shape used by [ADR 0023](0023-edit-standalone-calendar-appointments.md). Require a fresh provider read after confirmation and reject a changed identity, `changeKey`, or unsupported shape.
- Parse `@odata.etag` at the Graph boundary into a non-empty, valid HTTP header value. Keep the ETag private to the Graph adapter; use the domain event version to detect changes since the editor opened.
- Send one `DELETE /me/events/{id}` request with `If-Match` set to the ETag from the final GET. Do not retry the request. Treat `412 Precondition Failed` as a changed event, accept only `204 No Content` as confirmation, and treat a missing or unreadable response as an unknown outcome.
- Hold the account reservation from the final provider check through the delete request and cache transaction. After Graph confirms deletion, remove every cached row for that account and event ID in one SQLCipher transaction without changing calendar-view cursors.
- After an unknown delete result or a confirmed deletion that cannot be cached, require each previously synchronized view for that account to complete a successful sync before another calendar write. A newly created workflow also requires a successful sync before it permits a calendar write.
- Put Delete in the existing native editor and require a GPUI Kit `AlertDialog` naming the appointment with Cancel and Delete actions. Keep the editor open while the request is pending. Describe the existing delegated `Calendars.ReadWrite` permission accurately.
- Add no dependency or schema migration. Keep tenant proof in [issue #42](https://github.com/1fach/einfach-mail/issues/42), blocked by local implementation [issue #41](https://github.com/1fach/einfach-mail/issues/41).

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Use the final event ETag with `If-Match` | Lets Graph reject deletion when the event changed after the final read. | Requires a valid `@odata.etag`; live Exchange Online behavior remains unverified. |
| Compare `changeKey` before an unconditional DELETE | Uses the event version already captured by the editor. | Leaves a race between the final GET and DELETE. |
| Delete directly from the calendar row | Removes one interaction step. | Starts a destructive action before the editor has checked the provider's current event shape. |

## Consequences

- Application owns account reservation and recovery state; Graph owns event JSON, ETag validation, and HTTP status mapping; persistence owns the atomic multi-view cache removal; GPUI owns confirmation and visible pending or recovery states.
- Cache cursors remain unchanged after deletion. A subsequent sync reconciles the event's server-side removal.
- Synthetic tests can prove request construction, ETag handling, status mapping, reservation, cache behavior, and UI states. They cannot prove tenant permission consent or Exchange Online's conditional-delete behavior; keep both under issue #42.

## Sources

- [Microsoft Graph delete event v1.0](https://learn.microsoft.com/en-us/graph/api/event-delete?view=graph-rest-1.0)
- [Microsoft-maintained Graph v1.0 OpenAPI at revision ab170ed](https://github.com/microsoftgraph/msgraph-metadata/blob/ab170ed19a075b7b9df5a95bc4e08d9d49af56bd/openapi/v1.0/openapi.yaml)
- [Microsoft Graph get event v1.0](https://learn.microsoft.com/en-us/graph/api/event-get?view=graph-rest-1.0)
- [Microsoft Graph event resource](https://learn.microsoft.com/en-us/graph/api/resources/event?view=graph-rest-1.0)
- [OData Version 4.01 Protocol](https://docs.oasis-open.org/odata/odata/v4.01/odata-v4.01-part1-protocol.html)
- [ADR 0023: Edit standalone calendar appointments](0023-edit-standalone-calendar-appointments.md)
