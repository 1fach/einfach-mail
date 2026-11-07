# ADR 0050: Name GPUI DatePickers through a local component patch

- Status: Accepted
- Date: 2026-11-07

## Context

The event composer has separate `Starts` and `Ends` fields. Their styled GPUI Kit `DatePicker` controls are exposed as ComboBoxes without accessible names. The `DatePicker` implementation creates `gpui-base::DatePicker` internally. Base supports `aria_label`. GPUI Kit 0.8.1's styled component does not expose that property. GPUI Kit 0.9.0 still has no accessible-name builder in its source.

The app already uses the styled component for calendar selection, time editing, focus, keyboard actions, and popup behavior. Replacing it with Base would move this composition into the desktop app and duplicate behavior currently owned by GPUI Kit.

## Decision

Patch the existing Apache-2.0 `gpui-component` 0.8.1 package in `vendor/gpui-component`. Add an optional `accessibility_label` builder to its styled `DatePicker` and pass that value to the internal Base ComboBox. Name the event composer controls `Starts` and `Ends` through this builder.

Keep the package version and dependency graph unchanged. Keep the copied upstream license and source. Remove the local package patch when a published GPUI Kit release exposes the same builder and the headless regression test passes without the patch.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Patch the existing styled component | Keeps calendar, time, focus, and keyboard behavior in GPUI Kit without adding a runtime package or dependency edge | Copies about 4.5 MiB of the upstream component package and requires updating or removing the patch when GPUI Kit changes |
| Replace it with `gpui-base::DatePicker` | Base exposes `aria_label` directly and stays within GPUI Kit | The app would need to recreate the styled trigger, popup, calendar, time field, focus, and selection behavior |
| Wrap the styled picker in an app-owned labeled element | Avoids copying the crate | Names the wrapper rather than the focusable ComboBox, leaving the actual control unnamed |
| Keep the missing name and defer the issue | Avoids source maintenance | Leaves screen-reader users unable to distinguish start from end controls |

## Consequences

- The desktop application still depends on GPUI Kit as its only UI dependency.
- The local patch changes only the accessible name passed to the internal ComboBox. Without an explicit name, GPUI Kit rendering and behavior remain unchanged.
- The composer test queries the rendered accessibility tree for the `ComboBox` role and checks both distinct names.
- Recheck the styled `DatePicker` API when upgrading GPUI Kit. Remove the patch when an upstream release provides the same builder and the regression test passes without it.
- Headless tests do not prove the result in a real screen reader. Keep manual verification in issue #22.

## Sources

- [GPUI Kit 0.8.1 styled DatePicker source](https://github.com/longbridge/gpui-kit/blob/v0.8.1/crates/component/src/time/date_picker.rs)
- [GPUI Kit 0.8.1 Base DatePicker source](https://github.com/longbridge/gpui-kit/blob/v0.8.1/crates/base/src/date_picker.rs)
- [GPUI Kit 0.9.0 styled DatePicker source](https://github.com/longbridge/gpui-kit/blob/v0.9.0/crates/component/src/time/date_picker.rs)
- [GPUI Kit v0.9.0 release notes](https://github.com/longbridge/gpui-kit/releases/tag/v0.9.0)
