# ADR 0055: Defer DatePicker accessible names without local vendoring

- Status: Accepted
- Date: 2026-11-09
- Owners: Einfach Mail maintainers

## Context

ADR 0050 added an `accessibility_label` builder to GPUI Kit's styled DatePicker by patching and vendoring the full `gpui-component` 0.9.0 package. The vendor copy adds about 4.7 MiB of upstream code to name two event-composer controls. GPUI Kit 0.9.0 does not expose the builder, so keeping the names requires maintaining and verifying the whole local package.

The composer shows visible `Starts` and `Ends` labels. The focusable ComboBox controls lose those names when the local patch is removed. This is a known accessibility gap. [Issue #176](https://github.com/1fach/einfach-mail/issues/176) tracks an upstream API or a maintained smaller component that can name each control. Real-window screen-reader review remains under [issue #22](https://github.com/1fach/einfach-mail/issues/22).

## Decision

Remove the `gpui-component` patch, vendor tree, verification script, styled DatePicker label calls, and the headless assertion for the two names. Keep the published styled DatePicker, visible labels, and existing calendar, time, keyboard, selection, and appointment behavior. Accept the temporary absence of distinct accessible names and keep issue #176 open.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Keep the local package patch | Preserves distinct names on the actual focusable controls | Carries about 4.7 MiB of upstream source plus a checksum-and-patch maintenance script for two labels |
| Replace the styled picker with `gpui-base::DatePicker` | Base exposes an accessible-name API without a vendor copy | The application would need to recreate the styled calendar, time field, focus, keyboard, and selection behavior |
| Put the name on a surrounding element | Avoids changing the dependency | Names the wrapper rather than the focusable control and does not solve the accessibility problem |
| Defer the names and track a smaller follow-up | Removes the source maintenance burden while preserving the styled controls | Screen-reader users may not distinguish the start and end controls until issue #176 is resolved |

## Consequences

- Cargo uses the unmodified published `gpui-component` package; the local vendor tree and its validation script are deleted.
- The event composer keeps the visible `Starts` and `Ends` labels, but the accessibility tree no longer exposes them as the date-picker names.
- Issue #176 remains open for an upstream API or maintained smaller component. Do not claim distinct date-picker names until a headless assertion verifies both focusable controls.
- Issue #22 remains open for real-window screen-reader and other manual Linux accessibility checks.

## Sources

- [GPUI Kit 0.9.0 styled DatePicker](https://github.com/longbridge/gpui-kit/blob/v0.9.0/crates/component/src/time/date_picker.rs)
- [GPUI Kit 0.9.0 Base DatePicker](https://github.com/longbridge/gpui-kit/blob/v0.9.0/crates/base/src/date_picker.rs)
- [Issue #176: Name the event composer date pickers](https://github.com/1fach/einfach-mail/issues/176)
