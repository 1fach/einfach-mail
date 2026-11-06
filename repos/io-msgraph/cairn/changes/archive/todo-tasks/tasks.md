---
cairn: tasks
change: todo-tasks
---

- [x] Verify the open facts live on the test tenant (stash, categories, `mid:` linked resources, date truncation, nulls, delta rows)
- [x] Lists: `MsgraphTodoList` and its list, get, create, update, delete and delta coroutines
- [x] Tasks: `MsgraphTodoTask` and its list, get, create, update, delete and delta coroutines, checklist items listing
- [x] Blocking client methods for both
- [x] `ical` projection of a task, sharing the events projection's helpers
- [x] Unit tests for the projection, coroutine tests for the new requests
- [x] Live tests: list CRUD and delta, task CRUD and delta, projection round trip
- [x] Docs: src/lib.rs module layout, live test permissions, CHANGELOG.md
- [x] Fold into cairn/spec/todo.md, log, archive
