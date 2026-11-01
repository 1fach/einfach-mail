# ADR 0024: Navigate cached messages with a focused listbox

- Status: Accepted
- Date: 2026-11-01

## Context

The product brief includes keyboard navigation, and [issue #8](https://github.com/1fach/einfach-mail/issues/8) tracks cached message browsing. `MailView` owns the selected message ID and opens its body through `select_message`. The existing `v_virtual_list` renders large cached folders. Keeping these owners avoids a second selection state.

GPUI Kit 0.8.1 also provides a higher-level `List` with selection actions. Its `RowsCache` wraps Up from the first row to the last and Down from the last row to the first. Its delegate and `ListState` would add index-based selection beside `MailView.selected_message_id`.

## Decision

- Keep `v_virtual_list`. Keep message selection and body state in `MailView`. Add no dependency or index-based selection state.
- Give the message list one retained `FocusHandle` and a named `Role::ListBox` node. Give each message a `Role::ListBoxOption`, an accessible sender and subject, a position, a set size, and a selected state. Set `.aria_active_descendant()` on the selected option. GPUI treats it as focused while the listbox has focus.
- Bind Up, Down, Home, and End as typed actions in the message-list key context. They work only while the listbox has focus. Tab and Shift+Tab move focus out of the listbox.
- With no selection, Down selects the first row and Up selects the last. Home and End select the first and last rows. At either edge, Up and Down stop instead of wrapping. Empty or unavailable lists do not open a message.
- Route new selections through `select_message` to keep its body loading and error behavior. Scroll the selected row into view with `VirtualListScrollHandle::scroll_to_item` and `ScrollStrategy::Nearest`. Clicking an option opens it and returns focus to the listbox.
- Show a visible focus indicator and a shortcut hint beside the Messages heading. Headless UI tests use synthetic messages and cover focus, key actions, boundary behavior, clicking, and scrolling. They do not make live Graph requests or verify pixels or screen-reader output.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Keep `v_virtual_list` and add a focused listbox with contextual actions | Keeps virtual rendering, stable message IDs, and `MailView` selection state. Supports edge-stopping navigation and one Tab stop. | Adds keyboard and accessibility code to the app. |
| Replace it with GPUI Kit `List` | Uses a maintained component with built-in selection, roles, and scrolling. | The current component wraps at both ends and stores selection by index in a delegate. Matching this behavior would duplicate selection state or need a larger adapter and refactor. |
| Keep every message row as an independent tab-stop button | Keeps the current row component and pointer behavior. | Tab would visit every cached message. The list would not act as one control. |

## Consequences

- Keyboard navigation stays in `apps/desktop`. It changes no domain, application, provider, or persistence boundary and adds no dependency.
- A new selection opens the message body through the same path as a click. If the body is not cached, the existing workflow may fetch it from Graph. Rapid key repeats can request several bodies. `selected_message_id` decides which result the view applies.
- Headless UI tests check dispatched keys, focus ownership, selected state, and virtual scrolling. They do not verify Linux pixels or assistive technology. Issue #9 remains open for manual Linux checks.

## Sources

- [Issue #8: Browse folders and read synchronized mail offline](https://github.com/1fach/einfach-mail/issues/8)
- [GPUI Kit 0.8.1 List source](https://docs.rs/crate/gpui-component/0.8.1/source/src/list/list.rs)
- [GPUI Kit actions and keybindings guide](../../.agents/skills/gpui-kit/references/gpui/action.md)
- [GPUI Kit headless UI testing guide](../../.agents/skills/gpui-kit/references/gpui/test.md)
- [GPUI Kit keyboard accessibility design guide](../../.agents/skills/gpui-kit-design-guides/SKILL.md)
