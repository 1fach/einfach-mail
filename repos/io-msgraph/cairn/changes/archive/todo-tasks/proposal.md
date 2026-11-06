---
cairn: change
id: todo-tasks
status: landed
created: 2026-11-06
---

# Microsoft To Do tasks

## Why

The crate covers mail, contacts and calendars but not Microsoft To Do, where a Microsoft account keeps its tasks: a calendar there holds events only. The Android app asked for it (its GTD lists run on VTODOs and today only CalDAV calendars take them), and it needs the same shape it already syncs events through: typed coroutines, a delta round, and an `ical` projection that round-trips every line it writes.

## What

- **Lists** (`users.todo.lists`, Graph's `todoTaskList`): list, get, create, update, delete and delta, under `v1::rest::users::todo::lists`, the resource as `MsgraphTodoList`.
- **Tasks** (`users.todo.lists.tasks`, Graph's `todoTask`): list, get, create, update, delete and delta, under `v1::rest::users::todo::tasks`, the resource as `MsgraphTodoTask`. Its `@odata.etag` is exposed as the revision (todoTask has no `changeKey`). Linked resources come inline, read-only; checklist items are typed, returned when expanded, plus a listing coroutine; no attachments.
- **Projection** (`ical` feature): `MsgraphTodoTask::to_ical`, `from_ical`, `create_from_ical`, `update_from_ical` and `stashed_uid`, mirroring the events projection, every line without a Graph slot stashed verbatim in the same single-value extended property events use.

Facts verified live on the test tenant (2026-11-06), which shape the projection:

- todoTask carries `singleValueExtendedProperties` in v1.0: settable inline at create, replaced by a PATCH, read back on a get or a listing through `$expand`. A delta refuses that `$expand` (400 `initialState`), so a changed task is re-read by id.
- `categories` take any string (`inbox`, `@phone`), not only the user's Outlook categories.
- A linked resource keeps a `mid:` URI in `webUrl` verbatim. It can only be created inline at create or through its own endpoint, never by a PATCH of the task.
- `dueDateTime`, `startDateTime` and `completedDateTime` are truncated to midnight in the zone sent, and answered in UTC unless a recurrence names a zone or the request prefers one; `reminderDateTime` keeps its time.
- A PATCH takes explicit nulls (clearing) and a text body. A status set to completed without a date gets `completedDateTime` minted at midnight UTC.
- A recurrence comes back padded (`month: 0`, `dayOfMonth: 0`, `endDate: 0001-01-01`), and one set without a due date gets the due date minted from its start date.
- A delta answers removals as `{"id", "@removed": {"reason": "deleted"}}`, and takes several `Prefer` headers.

Projection rules:

- SUMMARY, DESCRIPTION (text; an HTML body is read as its text) and CATEGORIES map verbatim.
- PRIORITY 1 to 4 is high, 6 to 9 low, anything else normal; read back, high is 1, low 9, normal no PRIORITY.
- STATUS IN-PROCESS and COMPLETED map to theirs, NEEDS-ACTION or none to notStarted, CANCELLED to completed. waitingOnOthers and deferred, which RFC 5545 lacks, ride `X-MSGRAPH-STATUS` both ways, refining an open status only.
- DUE and DTSTART are dates: a DATE is written as midnight in the zone the caller passes, and a Graph value is read as the date of the nearest midnight in that zone, which holds whatever zone wrote it within twelve hours of the caller's. COMPLETED is the UTC instant Graph holds.
- RRULE maps onto the recurrence anchored on DUE (DTSTART without one), as To Do recurs on the due date; without either it is stashed. A weekly rule without WKST is sent starting on Monday (RFC 5545), which Graph would otherwise take as Sunday.
- The first DISPLAY or AUDIO VALARM with an absolute trigger is the reminder; read back as a DISPLAY alarm at that instant.
- A linked resource with a URL is minted as `ATTACH;X-MSGRAPH-LINKED-RESOURCE=<id>:<url>`, which a write drops: Graph keeps linked resources apart from the task.
- A managed line Graph holds coarser than written (a DUE with a time, a COMPLETED, PRIORITY 5, STATUS CANCELLED or NEEDS-ACTION, an RRULE in another spelling) is stashed too, and read back in place of the derived line while Graph's value still agrees with it, so the app's own document comes back as written and an edit made in To Do still wins.
- DTSTAMP, CREATED, LAST-MODIFIED and SEQUENCE are Graph's and consumed; UID and every other line (CONTACT, RELATED-TO, ATTACH, URL, X- properties, other alarms) are stashed.

## Out of scope

- Task attachments (`taskFileAttachment`), writing linked resources and checklist items: the app needs none of them.
- Mapping waitingOnOthers and deferred to GTD categories: the app's rule, not the crate's.
- An open extension: the MAPI stash works on todoTask, as on events and contacts.
