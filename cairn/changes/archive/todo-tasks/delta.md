---
cairn: delta
change: todo-tasks
---

# Delta

## ADDED Requirements

### Requirement: To Do lists and tasks are typed coroutines
The crate SHALL list, get, create, update, delete and delta-track To Do lists and the tasks of a list, list the checklist items of a task, and expose a task's and a list's `@odata.etag` as their revision. Delta rounds SHALL be continued and restarted from their links, and removals SHALL read as rows carrying `@removed`. A task's linked resources SHALL come inline with every read, and its checklist items when expanded; a task update SHALL write neither.

### Requirement: A task projects onto one VTODO and back
With the `ical` feature, a task SHALL project onto a VCALENDAR holding one VTODO, and a document SHALL project onto a full-state, a create and a delta update body, the update leaving Unset every managed field the base document holds the same. A document whose first component is not a VTODO SHALL be refused, and so SHALL an RRULE Graph has no recurrence pattern for.

### Requirement: Every line without a Graph slot survives
A line the projection does not map SHALL be stashed verbatim in the single-value extended property events use and restored on read, the UID among them, a stashed component whole. A managed line Graph holds coarser than written (a DUE with a time, a COMPLETED, a PRIORITY other than 1 and 9, a STATUS other than IN-PROCESS and COMPLETED, an RRULE spelled otherwise) SHALL be stashed too and restored in place of the derived line while Graph's value agrees with it, and dropped once it no longer does. An RRULE written without a DUE or a DTSTART to anchor it SHALL be stashed and restored while Graph holds no date and no recurrence.

### Requirement: Dates are told in the caller's zone
A DUE or DTSTART date SHALL be written as midnight in the zone the caller passes, and a Graph due or start SHALL read back as the date of the nearest midnight in that zone, which is the written date whatever zone within twelve hours wrote it. COMPLETED SHALL be written as its instant told in that zone and read back as the UTC instant Graph holds. The first DISPLAY or AUDIO alarm with an absolute trigger SHALL be the reminder.

### Requirement: Recurrences repeat on the due date
An RRULE SHALL map onto the recurrence anchored on DUE, on DTSTART without one, and read back from Graph's recurrence anchored on the due date. A weekly rule without WKST SHALL be sent starting its weeks on Monday.

### Requirement: Graph-only states round-trip
waitingOnOthers and deferred SHALL read back as `X-MSGRAPH-STATUS` beside no STATUS, and SHALL be written from it when the document's STATUS is absent or NEEDS-ACTION. notStarted SHALL read back as no STATUS, CANCELLED SHALL be written as completed, and importance normal SHALL read back as no PRIORITY.

### Requirement: Linked resources are read-only
A linked resource with a URL SHALL read back as an ATTACH carrying `X-MSGRAPH-LINKED-RESOURCE`, and a write SHALL drop such a line rather than stash it.

## MODIFIED Requirements

## REMOVED Requirements
