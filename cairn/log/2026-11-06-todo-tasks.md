---
cairn: log
change: todo-tasks
landed: 2026-11-06
---

# Microsoft To Do lists and tasks

The crate now covers Microsoft To Do, asked for by the Android app, whose GTD lists run on VTODOs that only CalDAV calendars could take: `v1::rest::users::todo::lists` (list, get, create, update, delete, delta over `MsgraphTodoList`), `v1::rest::users::todo::tasks` (the same over `MsgraphTodoTask`, plus the checklist items listing), the `MsgraphClientStd` methods, and behind `ical` the projection of a task onto one VTODO and back, mirroring the events one and sharing its helpers (made `pub(crate)` in src/v1/rest/users/events/ical.rs, `stash_lines` now taking the property field).

Facts verified live on the Microsoft test tenant before the design was fixed, the Graph reference staying silent or wrong on each:

- todoTask carries `singleValueExtendedProperties` in v1.0: set inline at create, replaced by a PATCH, read back through `$expand` on a get and on a listing. A task delta refuses that `$expand` (400 "Parsing OData Select and Expand failed: Value cannot be null. (Parameter 'initialState')"), so a consumer re-reads changed tasks by id, by `$batch` for the Android app.
- `categories` take any string, not only the user's Outlook categories; a linked resource keeps a `mid:` URI in `webUrl`.
- `dueDateTime`, `startDateTime` and `completedDateTime` are truncated to midnight in the zone sent and answered in UTC (or in a recurrence's zone, or the `Prefer: outlook.timezone` one); `reminderDateTime` keeps its time. A completion without a date gets midnight UTC.
- A PATCH takes explicit nulls and a text body (unlike Outlook events, no 500 on nulls). Every response carries `@odata.etag`, moving with each edit.
- A recurrence comes back padded (`month: 0`, `dayOfMonth: 0`, `endDate: 0001-01-01`), `firstDayOfWeek` defaulting to sunday; one set without a due date gets its start date as due date.
- A delta removal is `{"id", "@removed": {"reason": "deleted"}}`, with `isReminderOn` and `hasAttachments` defaulted beside.

The projection reads a due or start date as the nearest midnight in the caller's zone, so a date written by To Do in another zone, or at midnight UTC, still lands on its day; it keeps a managed line Graph holds coarser than written in the stash and restores it while Graph agrees, so the Android app's documents read back as written (covered by `a_written_task_reads_back_as_written` and the live `ical_todo`). Building it surfaced a bug in the events projection, fixed apart (2026-11-06-stashed-alarm-whole).

Live suites run on 2026-11-06: `todo`, `ical_todo`, and the events `ical`, `ical_series`, `ical_instances`, `ical_shapes`, all passing; the test app already held `Tasks.ReadWrite.All`.

Capabilities moved: todo (new).
