# ADR 0065: Define safe EWS calendar event writes

- Status: Accepted
- Date: 2026-11-12
- Owners: Einfach Mail maintainers

## Context

ADR 0057 keeps the EWS calendar read-only. Issue #389 defines a future, limited set of calendar actions for on-premises Exchange. ADR 0064 already models create, update, and delete as separate capabilities, and Graph implements those actions independently. The EWS provider must remain view-only until each action has an implementation that meets this policy and its live Exchange verification is complete.

EWS does not document a client idempotency key for `CreateItem`. A stable custom extended property can correlate a saved item with an app operation, but Exchange does not promise uniqueness for that property or that a zero-result search means a timed-out request cannot still commit. Replaying an uncertain create could therefore produce duplicates. Update and delete also need safeguards against changing meetings or overwriting another client's edit. The current typed EWS crate omits required operation fields and does not model several calendar fields; the existing EWS adapter already has bounded `quick-xml` handling for these wire gaps.

## Decision

- Limit the first EWS calendar actions to creating, updating, and deleting one-off, timed, standalone appointments in the primary calendar. Reject recurring, all-day, meeting, attendee, resource, online-meeting, and secondary-calendar shapes. EWS advertises each action only after that action has a reviewed implementation and the supported Exchange behavior has live verification. Until then, EWS remains view-only.
- Keep domain values provider-neutral. The domain owns the existing event creation, update, occurrence, and version values; it does not contain EWS XML or Graph types. The provider validates external item shape and maps a fully confirmed event into these domain values. `CalendarEventVersion` carries the opaque EWS `ChangeKey`.
- Keep durable create state and the retry decision in the application and cache boundaries. Before any remote request, persist the exact draft and stable operation ID in the encrypted cache. Change the cache staging result so it distinguishes a newly persisted operation from the same unresolved operation already present. For a new operation, the application calls the provider's create path once. For an existing unresolved operation, it calls a separate provider recovery path. Graph may replay its exact draft with the same `transactionId`; EWS recovery must search for the marker and must never send `CreateItem` again. A rejected marker must be resolved locally before another operation can be staged.
- For an EWS create, send one `CalendarItem` to the distinguished `calendar` folder. Include only the draft subject, UTC start and end, and a fixed app-owned named string extended property containing the stable operation ID. Use property-set GUID `4c2a2922-972e-54f9-bf4a-21e6b6d66d82`, property name `EinfachMailCalendarOperationId`, and `String` type; do not use a per-operation GUID or dynamic property tag. Set `SendMeetingInvitations="SendToNone"`; omit attendees, body, location, recurrence, all-day fields, meeting fields, and online-meeting data. Express times as UTC `Start` and `End` values without `TimeZoneContext`, `StartTimeZone`, or `EndTimeZone`, following Microsoft's documented UTC request shape.
- After an uncertain create, query only the primary calendar using an exact restriction on the same named property. Request enough results to detect duplicates, bounded to two matches, and require a complete search result. Treat a single match only as a candidate: fetch it by ID and validate its item identity, non-empty change key, exact operation marker, expected subject and UTC range, and supported standalone appointment shape. Reconcile only that validated item. Zero matches, multiple matches, incomplete results, an EWS error, or malformed or mismatched fields remain unresolved. A later lookup may reconcile the same operation if the item appears. Never retry or automatically delete a possible duplicate. A marker is a correlation aid, not an exactly-once guarantee; keep the unresolved operation and block another create until it is resolved.
- Before an update or delete, the EWS provider fetches the current item while the application holds the account reservation. It must confirm `CalendarItemType=Single`, `IsRecurring=false`, `IsAllDayEvent=false`, `IsMeeting=false`, and `IsOnlineMeeting=false`. Required, optional, and resource attendee collections must contain no entries; an omitted collection is empty. Missing or contradictory scalar fields, malformed collections, or any attendee entry make the item ineligible. Do not infer eligibility from the cached calendar summary.
- For updates, use the current `ItemId` and `ChangeKey`, `ConflictResolution="NeverOverwrite"`, and the required `SendMeetingInvitationsOrCancellations="SendToNone"`. Change only subject and UTC start/end. When either endpoint changes, send both. Do not update attendees, body, location, recurrence, all-day, or meeting data. A conflict is a known changed-event result. Read the item back after success and confirm its ID, subject, timed range, supported shape, and current change key before returning it to the application.
- For deletes, use `DeleteType="MoveToDeletedItems"` and the required `SendMeetingCancellations="SendToNone"`. Never use `HardDelete` or `SoftDelete`; reject meetings rather than sending cancellations. Validate the single per-item response status, but do not invent a returned item ID or change key. Moving an item changes its ID, so do not reuse the calendar ID as an ID in Deleted Items.
- Validate each EWS response message independently. For a one-item request require exactly one response with a matching operation and item count. Only `ResponseClass="Success"` with `ResponseCode=NoError` confirms success. A matching `Error` class with a non-`NoError` code is a definitive rejection. `Warning`, contradictory class and code, missing or extra response messages or items, malformed XML, and transport failure after dispatch do not confirm the write. Treat these outcomes as unknown and retain the existing recovery state.
- Create and update responses must contain the exact expected item kind, case-sensitive ID, and non-empty change key before the follow-up `GetItem`. `CreateItem` success does not include the created subject or times, so only the fully validated read-back is eligible for cache commit. `UpdateItem` also requires a read-back matching the requested fields; a successful no-op may retain the same change key. `DeleteItem` returns status only; cache removal is permitted only after exact `Success`/`NoError` confirmation.
- Keep update and delete uncertainty under the durable per-view calendar recovery rules in ADR 0052. Preserve recovery markers after unknown outcomes, stale-item conflicts, mismatched confirmations, and cache failures. Clear them only in the existing confirmed cache transaction or after successful synchronization.
- Keep request construction, XML shape checks, external response parsing, and EWS-specific error mapping inside `crates/providers/exchange-ews`. Continue using the typed `ews` crate and existing `quick-xml` 0.41.0 for narrowly scoped operations. Add no general SOAP model, parser dependency, fork, or vendored source. Keep provider XML and EWS types out of application and domain APIs.
- Give the user a recovery action that checks an unresolved EWS create's status. Do not label that action as a retry. The UI follows the provider-neutral pending state; it does not choose or implement EWS request policy.
- Synthetic SOAP fixtures must cover each action's success and explicit rejection, malformed or incomplete response, update conflict, timeout after server commit, cache failure, and create recovery after restart. Keep live Exchange verification separate. Synthetic fixtures do not prove interoperability with supported on-premises Exchange versions.

This decision plans milestone M11. It does not implement EWS writes or change their current capability. It narrows any future EWS calendar write scope to the supported item shapes above and leaves unsupported shapes read-only.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Correlate creates with a named property and only search during EWS recovery | Can positively reconcile a completed create without resubmitting an uncertain request. | A zero-result query remains inconclusive; unresolved operations can block further creates until later reconciliation. This is the selected policy. |
| Replay an uncertain EWS create with the same property value | Looks similar to Graph's transaction-ID recovery. | EWS does not document uniqueness or deduplication for custom properties, so it can create duplicates. |
| Implement the full EWS calendar item surface | Supports meetings, recurrence, all-day events, and secondary calendars. | Requires notification, time-zone, recurrence, and attendee rules beyond the current domain model and verification evidence. |
| Extend or fork `ews` to add every missing EWS model field | Could add typed fields to the dependency API. | Duplicates active upstream schema work for a small number of operation-specific gaps. The existing `quick-xml` boundary can handle those gaps without a new dependency or fork. |

## Consequences

- The application/cache contract must distinguish first submission from recovery of an existing unresolved creation. Graph recovery can retain its documented transaction-ID behavior; EWS recovery can only search.
- The EWS adapter must validate the standalone appointment shape from a fresh `GetItem`, because the current generic EWS `Message` type does not model `Start`, `End`, `IsMeeting`, or attendee collections.
- The cache continues to store the existing exact pending creation; no EWS credential, event body, attendee data, or raw SOAP is added to logs or fixtures. Update and delete use the existing persisted sync-recovery markers.
- An inconclusive create can remain unresolved indefinitely and block another create and account removal. The UI must explain this state and offer another status check; an abandonment or duplicate-acceptance action requires a separate decision and must never silently resubmit the create.
- M11 implementation can proceed from fixture-backed child issues while live Exchange verification remains independently tracked. EWS capabilities stay view-only until implementation review and live verification are complete.
- No live Exchange Server or sanitized live response recording is available. This decision accepts a fail-closed design, not a claim that EWS writes currently work or have been verified.

## Sources

- [Issue #389: Define safe EWS calendar event writes](https://github.com/1fach/einfach-mail/issues/389)
- [ADR 0052: Persist calendar event write recovery](0052-persist-calendar-event-write-recovery.md)
- [ADR 0057: Read-only EWS calendar snapshots](0057-read-only-ews-calendar-snapshots.md)
- [ADR 0064: Model calendar capabilities by operation](0064-calendar-capabilities-by-operation.md)
- [Microsoft: CreateItem for calendar items](https://learn.microsoft.com/en-us/exchange/client-developer/web-service-reference/createitem-operation-calendar-item)
- [Microsoft: CalendarItem fields](https://learn.microsoft.com/en-us/exchange/client-developer/web-service-reference/calendaritem)
- [Microsoft: Create UTC appointments with EWS](https://learn.microsoft.com/en-us/exchange/client-developer/exchange-web-services/how-to-create-appointments-in-a-specific-time-zone-by-using-ews-in-exchange)
- [Microsoft: FindItem](https://learn.microsoft.com/en-us/exchange/client-developer/web-service-reference/finditem-operation)
- [Microsoft: EWS restrictions](https://learn.microsoft.com/en-us/exchange/client-developer/web-service-reference/restriction)
- [Microsoft: IsEqualTo restriction](https://learn.microsoft.com/en-us/exchange/client-developer/web-service-reference/isequalto)
- [Microsoft: ExtendedFieldURI](https://learn.microsoft.com/en-us/exchange/client-developer/web-service-reference/extendedfielduri)
- [Microsoft: UpdateItem conflict and meeting-notification attributes](https://learn.microsoft.com/en-us/exchange/client-developer/web-service-reference/updateitem)
- [Microsoft: DeleteItem and recoverable deletion](https://learn.microsoft.com/en-us/exchange/client-developer/web-service-reference/deleteitem)
- [Microsoft: Verify individual EWS response messages](https://learn.microsoft.com/en-us/exchange/client-developer/exchange-web-services/verifying-the-results-of-an-ews-or-ews-managed-api-call)
- [Microsoft: EWS extended properties](https://learn.microsoft.com/en-us/exchange/client-developer/exchange-web-services/properties-and-extended-properties-in-ews-in-exchange)
- [`ews` calendar item model](https://github.com/thunderbird/ews-rs/blob/726235bc968b5273e06bf44c9b365f6631c88a63/ews/src/types/common.rs)
- [EWS calendar create and recovery dependency review](../project/dependency-review.md#ews-calendar-write-wire-support-2026-11-12)
