# ADR 0023: Edit standalone calendar appointments

- Status: Accepted
- Date: 2026-11-01

## Context

The product brief includes updating calendar events. M3 stores Graph-expanded events by account and fixed week. M4 already creates one-off timed events with delegated `Calendars.ReadWrite`, but [ADR 0016](0016-idempotent-primary-calendar-event-creation.md) explicitly excluded updates. The existing calendar composer owns the native subject and timed schedule controls; the Graph adapter owns event JSON and the encrypted cache owns one row per event per synchronized range.

Microsoft Graph v1.0 updates an event with `PATCH /me/events/{id}`. It applies only supplied properties and returns `200 OK` with an updated event. The event resource exposes `type`, `isOrganizer`, `attendees`, `isOnlineMeeting`, `isAllDay`, and `changeKey`; Microsoft says `changeKey` changes whenever the event changes. Neither the update documentation nor Microsoft's maintained v1.0 OpenAPI contract declares conditional `If-Match` support for PATCH. An immediate version re-read can detect edits made while the user had the form open, but cannot close the race between that read and PATCH.

Changing attendee data can send meeting updates. Updating an online meeting body can remove its meeting blob. Recurrence instances and all-day events also need rules that the current editable schedule model does not own. There is no test tenant, so the implementation must rely on synthetic responses and leave live behavior as a separate acceptance check.

## Decision

- Offer editing for a cached event only after a fresh Graph read confirms all of the following: `type` is `singleInstance`, `isOrganizer` is `true`, `attendees` is present and empty, `isOnlineMeeting` is `false`, `isAllDay` is `false`, and the event has a valid `changeKey`, subject, start, and end. Refuse missing or contradictory metadata.
- Use a native GPUI form to change only the subject and timed start/end. Keep recurrence, attendees, location, body, online-meeting data, and deletion out of this action. Convert edited local times with Jiff and the Linux system zone. Reject DST gaps and folds under ADR 0016. Existing event edits may move outside the currently displayed week; event creation keeps its current week limit.
- Capture the event's `changeKey` when opening the form. Immediately before a save, GET the event again and refuse the update if the key changed or eligibility no longer holds. This detects stale forms but is not an atomic concurrency guarantee: Graph's published event-update contract and OpenAPI do not document a conditional header, so a change in the final GET-to-PATCH interval remains possible.
- Send one Graph v1.0 PATCH without automatic retry. Include `subject` only when changed. When either endpoint changes, send both `start` and `end` as UTC `dateTimeTimeZone` values. Omit every other property. Require the exact `200 OK`, same immutable event ID, and requested values before reporting success. Treat a lost or unreadable response as uncertain and ask the user to sync before deciding whether to edit again.
- Model the checked edit snapshot and update as domain values. The application owns account-level calendar reservations during preflight and save, so no view delta or event creation can race with the update transaction. Do not hold a reservation while the user edits the form.
- After confirmation, remove all cached copies of that event and insert the updated occurrence into every already-cached overlapping primary-calendar range in one SQLCipher transaction. If the cache transaction fails, report the confirmed remote result and ask the user to sync. A later delta sync remains authoritative.
- Keep the existing delegated `Calendars.ReadWrite` permission and its disclosure. Do not add a Graph SDK or another dependency: the current Reqwest adapter already owns authority validation, redacted errors, bounded responses, token isolation, and non-retried writes.
- Keep real-tenant proof separate in [issue #40](https://github.com/1fach/einfach-mail/issues/40), blocked by local implementation [issue #39](https://github.com/1fach/einfach-mail/issues/39). Synthetic tests do not establish live consent or Graph behavior.

This decision supersedes the event-update exclusion in ADR 0016 only for the standalone, timed appointment defined above. It does not alter event creation, M3 recurrence reading, or the calendar permission request.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Check eligibility on demand and keep the snapshot in the open editor | Avoids a schema migration and does not persist invitee or organizer metadata. A fresh read protects against a form left open after a server change. | Adds two Graph reads around the edit; the final read and PATCH still have a narrow race because Graph does not document conditional updates here. |
| Persist edit eligibility in every calendar occurrence | Makes eligible actions immediately visible offline and avoids the first read. | Adds provider metadata and a persistence migration, exposes stale editability, and still requires a fresh read before writing. |
| Update only the visible week cache | Smallest storage change. | Leaves stale or duplicate event rows in other cached weeks, especially when an event moves across a range boundary. |
| Rebuild every cached range in one account transaction | Keeps all known cached weeks consistent and makes a confirmed update visible or absent atomically. | Requires short account-level exclusion while the remote write and cache transaction run. |
| Allow any Graph event type and preserve whatever fields are returned | Covers more existing calendar data. | Risks recurrence changes, all-day conversion, attendee notifications, and online-meeting metadata loss without a sufficiently rich editable model. |

## Consequences

- The domain gains checked-edit and update values; the application provider and cache ports gain corresponding operations and account-level coordination.
- The Graph adapter gains bounded event GET and non-retried PATCH operations. It validates editability at the network boundary and never stores attendee content in the domain or logs.
- The encrypted store changes all known copies of a confirmed event transactionally. No table migration or new dependency is required.
- Updating a schedule sends UTC instants, consistent with the calendar occurrence model. A changed event's source time-zone metadata may therefore become UTC; unchanged times are omitted from the PATCH and retain their server value.
- A `changeKey` comparison is a best-effort stale-form check, not server-enforced optimistic concurrency. The remaining narrow race is accepted only for this restricted no-attendee, non-online, single-instance appointment flow and must be examined with tenant evidence.
- Local acceptance covers synthetic provider contracts, workflow concurrency, encrypted multi-range cache behavior, and headless native UI states. No live Graph request is made.

## Sources

- [Microsoft Graph update event v1.0](https://learn.microsoft.com/en-us/graph/api/event-update?view=graph-rest-1.0)
- [Microsoft-maintained Graph v1.0 OpenAPI at revision ab170ed](https://github.com/microsoftgraph/msgraph-metadata/blob/ab170ed19a075b7b9df5a95bc4e08d9d49af56bd/openapi/v1.0/openapi.yaml)
- [Microsoft Graph event resource](https://learn.microsoft.com/en-us/graph/api/resources/event?view=graph-rest-1.0)
- [Microsoft Graph get event v1.0](https://learn.microsoft.com/en-us/graph/api/event-get?view=graph-rest-1.0)
- [ADR 0006: Graph HTTP transport](0006-graph-http-transport.md)
- [ADR 0014: Linux read-only primary calendar](0014-linux-read-only-calendar-workflow.md)
- [ADR 0016: Create one-off timed events](0016-idempotent-primary-calendar-event-creation.md)
- [Issue #39: Edit standalone calendar appointments](https://github.com/1fach/einfach-mail/issues/39)
- [Issue #40: Verify event updates in a Microsoft 365 test tenant](https://github.com/1fach/einfach-mail/issues/40)
