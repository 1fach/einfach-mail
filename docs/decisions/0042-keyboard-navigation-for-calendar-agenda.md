# ADR 0042: Use one listbox for weekly agenda navigation

- Status: Accepted
- Date: 2026-11-05
- Owners: Project maintainer

## Context

ADR 0014 defines a read-only weekly agenda. The agenda shows one occurrence on each local day it occupies, so a multi-day event can appear more than once. Keyboard navigation must identify a specific displayed occurrence and let the user move through the full week without adding a Tab stop for every event.

The GPUI Kit list component owns index-based selection and does not provide the required no-wrap navigation for this grouped agenda. GPUI provides focus handles, listbox and option roles, active-descendant reporting, and scroll handles for the behavior the agenda needs.

## Decision

- Render the week as one native GPUI listbox with one Tab stop. Day headings and empty-day messages are not selectable options.
- Use Up and Down to move between event occurrences without wrapping. When no occurrence is selected, Up selects the last occurrence and Down selects the first. Use Home and End to select the first and last occurrence.
- Identify an option by its local date and provider event ID. The date distinguishes repeated rows for a multi-day event.
- Expose each event as a listbox option with its date, time, subject, and location. Report the selected option through GPUI's selected and active-descendant accessibility state.
- Use one GPUI scroll handle for the list and its rows. Keep the selected option visible as the user navigates. Clicking an option selects it and returns keyboard focus to the list.
- Keep the agenda read-only. Do not add create, edit, or delete actions to M3.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| One listbox for the week | One Tab stop, one set of week-wide keyboard commands, and one selected occurrence at a time. | The application must keep selection, row identity, focus, and scrolling in sync. |
| A focusable control for every event | Each event is directly reachable through Tab. | A week with many events creates many Tab stops and does not meet issue #22's navigation criteria. |
| A separate list for each day | Each day's events have a local list. | Keyboard movement stops at day boundaries and requires extra focus transitions to reach the next day's events. |

## Consequences

- Headless GPUI tests cover the Tab stop, keyboard boundaries, visible selection, click focus, and scrolling with synthetic events.
- Screen-reader announcements, focus appearance, selection behavior, scrolling, and pixel layout still require manual review in a Linux desktop session.
- This interaction adds no dependency and makes no provider or persistence change.
- Event editing remains outside the M3 read-only scope.

## Sources

- [ADR 0014: Implement a cached Linux read-only primary calendar](0014-linux-read-only-calendar-workflow.md)
- [ADR 0024: Provide keyboard navigation for cached messages](0024-keyboard-navigation-for-cached-messages.md)
- [GPUI Kit focus handle guide](../../.agents/skills/gpui-kit/references/gpui/focus-handle.md)
- [GPUI Kit actions guide](../../.agents/skills/gpui-kit/references/gpui/action.md)
- [GPUI Kit test guide](../../.agents/skills/gpui-kit/references/gpui/test.md)
