# ADR 0066: Use a Fluent visual language and a destination switcher

- Status: Accepted; implementation tracked in issues #452–#457
- Date: 2026-11-13
- Owners: Einfach Mail maintainers

## Context

The repository owner wants Einfach Mail to look and feel like a Windows 11 app, modeled on Wino Mail and the Windows Mail client. The work uses only data the app has today. Preview text, unread counts, folder kinds, attachments, and importance follow in separate issues ([#458](https://github.com/1fach/einfach-mail/issues/458)–[#462](https://github.com/1fach/einfach-mail/issues/462)), each with its own ADR.

[ADR 0059](0059-native-mail-and-calendar-workspace.md) fixes the shared workspace and says to show navigation selection with the selected item's own surface and foreground, not a one-sided accent stripe. It keeps the app destinations inside the one contextual pane. The design guide also warns against edge bars. The Fluent navigation view marks the selected item with a short accent pill on a filled surface, and it places the destination icons at the bottom of the pane.

Today the desktop uses GPUI Kit defaults and one runtime patch. `Workspace::apply_appearance` calls `Theme::change` and then sets `ring = primary`. Every screen builds its own header, card, notice, and selection styling.

Constraints from the code:

- `Theme::change` reloads `Theme.light_theme` and `Theme.dark_theme`. A patch made after the reload must be repeated on every change. A config installed in those two fields applies on every change.
- `ThemeConfig` expresses every color role the design needs, plus `radius` and `radius.lg`. An unparsable color falls back to the default without an error, so tests must pin the values. `list.active.background` is clamped to an alpha of 0.2 or less, so the selected-row fill must be a translucent accent tint.
- `gpui_kit::assets::Assets` embeds 106 icons. It lacks Reply, Archive, Flag, Trash, MailOpen, FolderInput, and SquarePen. `AllAssets` embeds all 1,830 Lucide icons (about 749 KB of SVG).
- `gpui_kit::assets::icon_assets!` embeds a named subset as a separate `AssetSource`. The gpui-kit-assets `extra_assets` example composes it with `Assets`.
- `SidebarMenuItem` has no focus handle, debug selector, or pill, and it exposes tree-item semantics. It cannot keep the `nav-*` focus and test contract.
- [ADR 0024](0024-keyboard-navigation-for-cached-messages.md) and [ADR 0042](0042-keyboard-navigation-for-calendar-agenda.md) fix the listbox contract for message and agenda rows.

The full design, token table, and per-screen layout live in [the Fluent workspace design](../design/fluent-workspace.md).

## Decision

1. **Palette.** One ThemeSet file, `apps/desktop/src/components/theme/einfach-fluent.json`, holds the "Einfach Fluent Light" and "Einfach Fluent Dark" themes. It is the only place in `apps/desktop/src` with raw colors. `components/theme.rs` installs both themes once, in `initialize_application` before the window opens, as `Theme.light_theme` and `Theme.dark_theme`. `Workspace::apply_appearance` keeps only `Theme::change(mode)`. The `ring = primary` patch is deleted because `ring` is in the JSON. No second writer of theme state exists. A unit test fails when `apps/desktop/src` contains `rgb(`, `rgba(`, `hsla(`, or a `#RRGGBB` literal outside the theme files.
2. **Shape.** Radius is 4 px for controls, rows, and nav items. `radius.lg` is 8 px for cards, notices, and popovers. The base font stays 16 px and uses the platform UI font. Spacing keeps the GPUI rem scale. Only overlays carry a shadow.
3. **Selection.** The selected navigation item uses a filled rounded surface plus a short, centered, fully rounded accent pill inside the item's silhouette. The pill is 3 px by 16 px at the base size. It does not touch the item corners and does not form a second edge, so it differs from the full-height one-sided border that ADR 0059 and the design guide reject. Message and agenda rows use fill only. Unread state never relies on color alone: it adds semibold weight and an accent dot, and the row's accessible description says "Unread".
4. **Destinations.** A bottom icon switcher for Mail, Calendar, Accounts, and Settings sits inside the one contextual pane, above a hairline. It is not a fourth column. Each item has a tooltip and an accessible name. The `nav-*` ids and focus handles stay. The brand row goes, because the system title bar names the app. The navigation pane is a fixed 16 rem with no icon rail.
5. **Compose.** Compose moves from the Mail header into the pane as a primary "New mail" button, keeping the `new-mail-message` id. Delete becomes a neutral ghost button because it moves the message to Deleted Items and can be undone.
6. **Icons.** One icon family, Lucide, through an app `AppIcon` enum. An app `AssetSource` composes `gpui_kit::assets::Assets` with an `icon_assets!` set listing only the icons `AppIcon` needs. The full `AllAssets` bundle is not used.
7. **Primitives.** Feature code builds screens from small app components (navigation item, page, settings card, info bar, command bar, group header, list option, pane header) in `apps/desktop/src/components/`. Only these primitives read theme slots. Status tints come from one `status_colors` function.
8. **Scope.** This decision adds no data. People, To Do, Focused/Other, search, select-all, a rich-text toolbar, and an inert hamburger stay out. Translucency stays the opt-in from [ADR 0059](0059-native-mail-and-calendar-workspace.md) and issue #274.

This ADR amends the navigation-selection bullet and the destination placement in ADR 0059. The rest of ADR 0059 stands.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Registered ThemeSet installed once, fill plus pill selection, bottom switcher, curated icon subset | One owner for the palette. Appearance changes need no extra code. Matches the Windows Mail reference. The icon set stays small. | Departs from ADR 0059's selection rule, so this ADR amends it. Tests that rely on default theme values need updating. Each new icon needs one line in the `icon_assets!` list. |
| Fill-only selection (ADR 0059 as is) | No amendment. Matches the design guide. | Loses the cue that makes Fluent navigation recognizable. |
| Separate app rail beside the folder pane | Separates destinations from folders. | Four permanent regions leave less room for the message list and reader. ADR 0059 already rejected it. |
| gpui-component `Sidebar` | Ready-made. | No focus handle, debug selector, or pill, and tree-item semantics. It would break the `nav-*` contract. |
| Runtime `Theme::update` patches after each `change` (the current pattern, extended) | No new files. | Two writers of theme state. Every new role needs a repeat call. Easy to forget on a new appearance path. |
| Full `AllAssets` bundle | No icon list to maintain. | Embeds about 749 KB of SVG for 22 icons. |

## Consequences

- Every color in `apps/desktop/src` comes from the theme JSON. A source scan test enforces it. Light and dark values are pinned by exact-value and contrast tests, because a bad color silently falls back to the default.
- GPUI Kit paints a half-opacity halo around the focus ring, and the existing accessibility test requires that halo to keep 3:1 contrast against every surface. The JSON therefore sets `ring` apart from the accent. It is a deep navy in light mode and a slightly lighter blue in dark mode. Control outlines (`input.border`) use a stronger stroke than hairlines so unselected radios and inputs keep 3:1.
- Existing layouts pick up the palette in the foundation unit and change structure in later units (#453–#457). Behavior, ids, roles, labels, and keyboard paths stay unchanged. Tests that read row heights or click collapsed actions get deliberate updates in those units.
- The selected row fill is a translucent accent tint, because GPUI Kit clamps `list.active.background` to an alpha of 0.2.
- Message times use a 24-hour clock with English date labels until localization exists.
- Wino Mail is GPL-3.0 and is a visual reference only. No source or asset is copied.
- The design depends on a GPUI Kit pin. Re-run the theme tests on each GPUI Kit upgrade.
- Verify by running `scripts/preview-shots.sh` on Hyprland and by reading the screenshots under `docs/project/evidence/`, plus the headless desktop tests.

## Sources

- [Fluent workspace design](../design/fluent-workspace.md) and its [synthesis](../design/fluent-workspace/synthesis.md)
- [ADR 0059: Use a shared, adaptive mail and calendar workspace](0059-native-mail-and-calendar-workspace.md)
- [Wino Mail source repository](https://github.com/bkaankose/Wino-Mail), used as a reference only
- [GPUI Kit design guides](../../.agents/skills/gpui-kit-design-guides/references/design-guides.md)
- `gpui-component` 0.9.1 `ThemeConfig` and `Theme::change`, and `gpui-kit-assets` 0.9.1 `icon_assets!`, read from the locked crate sources
