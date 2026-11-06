---
cairn: spec
capability: events-ical
status: current
---

# Events iCalendar projection

The `ical` projection of a Graph event onto iCalendar and back. This file grows by accretion: it states the requirements changes have touched so far, the src/v1/rest/users/events/ical.rs header holding the rest.

### Requirement: A stashed component reads back whole
A component the projection stashes (an alarm Graph's single reminder cannot hold) SHALL read back as its lines in order, from its BEGIN to its END, and each stashed property SHALL be restored on its own, so one that no longer tokenises restores nothing rather than corrupting the event.
