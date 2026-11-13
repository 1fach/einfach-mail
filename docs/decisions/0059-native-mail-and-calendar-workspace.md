# ADR 0059: Use a shared, adaptive mail and calendar workspace

- Status: Accepted; the navigation selection and destination placement are amended by [ADR 0066](0066-fluent-visual-language-and-navigation.md)
- Date: 2026-11-10
- Owners: Einfach Mail maintainers

## Context

The Linux desktop currently opens Accounts first. It replaces that view with either Mail or Calendar, and each child view adds its own header and return button. The root `Workspace` owns the shared `AccountSession`; the mail and calendar views receive workflow-specific session ports. The accepted session boundary must remain intact when navigation changes. See [ADR 0043](0043-per-account-mail-and-calendar-session.md).

The supplied Wino Mail screenshots show persistent navigation, a dense message list beside a reading pane, and separate account and preference sections. Use them as references for hierarchy and density, not as pixel specifications. The screenshots use a dark theme and a custom Windows title bar; the Linux app follows GPUI Kit themes and system window decorations. The project already has native mail reading, a weekly calendar, account connection, and account details. The first release supports one active account at a time; see [ADR 0060](0060-one-active-account-first-release.md). Global mail search, People, and To Do remain out of scope.

The design should borrow the Windows mail workspace's clear hierarchy and density while keeping Linux window decorations and keyboard behavior. Windows materials do not map directly to Linux. Microsoft describes Mica as an opaque window material and Acrylic as a translucent material for transient surfaces. GPUI Kit 0.9 uses GPUI Fast 0.1.13, which exposes whole-window `Opaque`, `Transparent`, and `Blurred` backgrounds. On Linux, Wayland blur depends on compositor protocol support, while X11 exposes transparency without an application-controlled blur effect. GPUI does not provide per-element desktop backdrop blur.

## Decision

- Keep one shared desktop window with persistent destinations for Mail, Calendar, Accounts, and Settings. Show a destination only when it has working behavior. Add Settings to the visible navigation with the first persisted preference in issue #273. Store app-wide appearance in the Linux XDG config directory, separate from account credentials and the encrypted mail cache; System follows the active window's appearance. Use the system title bar on Linux.
- Use a Wino-inspired mail workspace with one contextual navigation pane, a message list, and a reading pane. Put the current account identity and its folders in the contextual pane. Keep the app destinations in that pane rather than adding a fourth permanent column.
- Use GPUI Kit semantic theme tokens in light and dark appearances. Keep hierarchy clear through spacing and grouping before color. Show navigation selection with the selected item's own surface and foreground, not a one-sided accent stripe.
- Give the main regions minimum, preferred, and maximum widths. At narrow widths, collapse the contextual pane or reading pane while preserving a clear keyboard path back to it. Keep selection and message identity stable when panes change.
- Keep the one-active-account scope from [ADR 0060](0060-one-active-account-first-release.md). Show the active account identity without a selector or add-account control.
- Place the existing weekly agenda inside the shared shell. Preserve its one-listbox keyboard behavior from [ADR 0042](0042-keyboard-navigation-for-calendar-agenda.md).
- Use GPUI Kit semantic theme tokens for surfaces and text. Keep message text and large work panes opaque. Limit translucency to transient overlays and compact navigation surfaces where it helps show their relationship to the content. Use a maintained GPUI API for any material effect. Defer the effect if Linux support needs a renderer fork or custom blur code.
- Treat a Linux blurred window backdrop as an optional platform effect, not a requirement. Keep an opaque fallback for unsupported compositors and transparency settings. Do not rely on X11 blur or ship a window effect before it has been reviewed in a real Linux session.
- Show only destinations and controls that perform a supported action. Defer People, To Do, global search, and settings that do not have real persisted behavior.

## Alternatives

| Layout | Benefits | Costs |
| --- | --- | --- |
| One Wino-style contextual pane, message list, and reader | Keeps the main mail task visible and fits the supplied screenshots without adding another permanent column. | The pane must combine app destinations with the current account's folders. Its narrow-width behavior needs deliberate design. |
| Separate app rail, account/folder pane, message list, and reader | Separates global destinations from mail folders. | Four permanent regions leave less room for the message list and reader at ordinary desktop sizes. |
| Two-pane list and separate message screen | Works at narrow widths with little resizing. | Hides the message list while reading and makes desktop scanning slower. |

Choose the first layout for a comfortable desktop window. Use the third layout as the narrow-window fallback. Do not keep the second layout expanded at all widths.

## Consequences

- The app shell owns destination selection, session-open errors, and the shared account session. The account feature owns account operations and requests sign-out through the workspace. Mail and Calendar remain application workflow clients.
- Mail and Calendar share navigation and window chrome. Their feature state stays in their existing views until implementation shows a clear owner for a shared state value.
- Mail pane splits need user-resizable limits and a sensible compact-width transition. Persist split sizes only after the layout proves they represent a useful user preference.
- Account navigation shows the one active identity and cannot imply that multiple accounts can be connected or switched. Issue #272 owns the account-detail UI.
- Translucency is progressive enhancement. If GPUI or the compositor cannot provide it, the app uses the same theme-aware opaque surface and remains readable.
- Real-window layout and compositor behavior still need local Linux review. Do not use real mailbox or calendar data for screenshots or tests.

## Implementation sequence

The [synthetic layout comparison](../design/native-workspace-options.svg) sketches the three options considered here. Track acceptance criteria in [M10 milestone 9](https://github.com/1fach/einfach-mail/milestone/9):

1. [Issue #268](https://github.com/1fach/einfach-mail/issues/268) records the one-active-account first-release decision.
2. [Issue #269](https://github.com/1fach/einfach-mail/issues/269) establishes persistent navigation and is the first implementation issue.
3. [Issues #270 and #271](https://github.com/1fach/einfach-mail/milestone/9) move Mail and Calendar into the shell.
4. [Issues #278 and #279](https://github.com/1fach/einfach-mail/milestone/9) handle narrow Mail and Calendar layouts after their desktop layouts.
5. [Issue #272](https://github.com/1fach/einfach-mail/issues/272) presents connected-account details. [Issue #280](https://github.com/1fach/einfach-mail/issues/280) presents account setup and permission review.
6. [Issue #273](https://github.com/1fach/einfach-mail/issues/273) adds the first working Settings destination and appearance preference.
7. [Issue #274](https://github.com/1fach/einfach-mail/issues/274) evaluates selective Linux translucency after the shell and pane work.

## Sources

- [Wino Mail source repository](https://github.com/bkaankose/Wino-Mail), used as a layout reference only
- [Microsoft: Mica material](https://learn.microsoft.com/en-us/windows/apps/design/style/mica)
- [Microsoft: Acrylic material](https://learn.microsoft.com/en-us/windows/apps/design/style/acrylic)
- [Microsoft: Windows app design best practices](https://learn.microsoft.com/en-us/windows/apps/get-started/best-practices)
- [GPUI Fast `WindowBackgroundAppearance`](https://docs.rs/gpui-fast/0.1.13/gpui/platform/enum.WindowBackgroundAppearance.html)
- [GPUI Fast Linux Wayland window source](https://github.com/longbridge/gpui-fast)
- [GPUI Kit issue #3598: Proposal for in-window backdrop materials](https://github.com/longbridge/gpui-kit/issues/3598)
- [GPUI Kit design guides](../../.agents/skills/gpui-kit-design-guides/references/design-guides.md)
- [GPUI Kit coding guides](../../.agents/skills/gpui-kit/references/coding-guides.md)
- [ADR 0042: Use one listbox for weekly agenda navigation](0042-keyboard-navigation-for-calendar-agenda.md)
- [ADR 0043: Use one per-account mail and calendar session](0043-per-account-mail-and-calendar-session.md)
- [ADR 0060: Support one active account in the first release](0060-one-active-account-first-release.md)
