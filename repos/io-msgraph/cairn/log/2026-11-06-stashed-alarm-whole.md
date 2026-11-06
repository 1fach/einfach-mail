---
cairn: log
change: stashed-alarm-whole
landed: 2026-11-06
---

# A stashed alarm reads back whole

`MsgraphEvent::to_ical` restored the stash one line per `push_raw` call. ical-rs places a property line before the first stashed `BEGIN`, so a stashed alarm (any alarm past the first, or one Graph's reminder cannot hold) came back with its BEGIN:VALARM at the end and its other lines spilled into the VEVENT: the read document was corrupt. The stash is now restored by `push_stash`, each property alone (a line that no longer tokenises still restores nothing) and each stashed component in one call. Shared with the To Do projection (2026-11-06-todo-tasks), which surfaced it.

Regression test `a_stashed_alarm_reads_back_whole`, failing on the previous code (the EMAIL alarm's ACTION, TRIGGER and SUMMARY came out above a dangling BEGIN:VALARM). Trivial fix, landed without a proposal.

Capabilities moved: events-ical (new file, by accretion).
