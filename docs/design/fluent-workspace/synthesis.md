# Fluent restyle: synthesized design (arena result)

Base: candidate A ([candidate A](candidate-a.md)). Read it in full; everything there holds unless changed below.
Grafted from candidate B ([candidate B](candidate-b.md)):

1. **Theme install once at startup** (B). `components/theme.rs::install(cx)` runs in `initialize_application` before the window opens. It parses the ThemeSet JSON and sets `Theme.light_theme` / `Theme.dark_theme` once. `Workspace::apply_appearance` keeps only `Theme::change(mode)`. The runtime `ring = primary` patch is deleted because `ring` is in the JSON. There is no second writer of theme state.
2. **Curated icon subset** (B), not `AllAssets`. An app `AssetSource` composes `gpui_kit::assets::Assets` with an `icon_assets!` set (or the crate's equivalent mechanism; verify the real API in gpui-kit-assets 0.9.1 before use). It adds only the Lucide icons `AppIcon` needs. Record the choice in `docs/project/dependency-review.md`. If `icon_assets!` does not exist or cannot compose, stop and report. Do not switch to `AllAssets` silently.
3. **No-raw-color source scan test** (B). A desktop unit test fails if `apps/desktop/src` contains `rgb(`, `rgba(`, `hsla(` or a `#RRGGBB` literal outside the theme JSON and `components/theme.rs`.
4. **Unread accent dot** (B), in addition to semibold weight. Unread is never shown by color alone. `aria_description` carries "Unread".

Rejected from B, with reasons:
- The `Fluent` role accessor and `FluentTheme` trait. They add a second vocabulary over theme slots (a pass-through layer). Feature code reads the existing semantic slots, and only primitives read them.
- The InfoBar close button. No notice can be dismissed today, so the control would not work.
- Changing option `aria_label` to include the time. A's `aria_description` keeps the existing label assertions.
- Width-computed `visible_count` overflow. A's single 40rem reader threshold is simpler.

Kept from A, where B differed:
- `InfoBar::status` and `InfoBar::alert` constructors that keep each notice's current role.
- `NavItem::unavailable(Option<&'static str>)`.
- The shared `list_option` (ADR 0024/0042 contract).
- `LocalClock` moved out of `calendar/week.rs`.
- `PaneHeader` (B's `ViewHeader`).
- Delete becomes a neutral ghost button because it is reversible.

Defaults chosen for A's open questions (reported to the user; easy to change):
- Base font stays 16px.
- Fixed 16rem nav, no rail mode.
- Times use a 24-hour clock with English date labels until localization.
- Delete is neutral.
- Curated icon subset instead of `AllAssets`.

## Implementation units (from A's decomposition, adjusted)

| # | Unit | Files | Depends on |
|---|---|---|---|
| U1 | ADR 0066 (Fluent visual language + nav amendment of 0059), dependency-review row for icon subset | docs/decisions, docs/project/dependency-review.md, docs/decisions/README.md | none |
| U2 | Preview lever: `EINFACH_MAIL_UI_PREVIEW_DESTINATION`, `_APPEARANCE`, `_WINDOW_SIZE` (ui-preview only), `scripts/preview-shots.sh` (Hyprland `hyprctl` + `grim`) | app/bootstrap.rs, app/preview.rs, app/workspace.rs (cfg ui-preview builder), scripts/, docs/project/setup.md | none |
| U3 | Token layer: theme JSON, `components/theme.rs` (install, StatusTone, status_colors), `components/icon.rs` (AppIcon + curated assets), remove ring patch, exact-value + contrast + no-raw-color tests | components/, app/bootstrap.rs, app/workspace.rs apply_appearance, app/workspace/tests/appearance.rs | U1 |
| U4 | Shell: NavItem (pill), destination switcher, account header with Avatar, New mail moved to MailFolderNavigation, `components/shell.rs` NAVIGATION_PANE_WIDTH, window min size | app/workspace/render.rs + state.rs, mail/view/folder_navigation.rs, mail/view/render.rs (header button removal), mail/view.rs constant | U3 |
| U5 | Pages: Page, SettingsCard, InfoBar; Settings + Accounts migrated; workspace stops centering Accounts on both axes | features/settings/*, features/account/render.rs, app/workspace/render.rs (`render_account_workspace_content` only) | U3 |
| U6 | Mail list: LocalClock, day_groups, GroupHeader, list_option, PaneHeader, row anatomy, row sizes | mail/view.rs, mail/view/message_list.rs, mail/view/render.rs (header), components/ | U4 |
| U7 | Reader CommandBar with overflow | mail/view/message_reader.rs, message_actions.rs, read_state.rs, follow_up_flag.rs | U4 |
| U8 | Calendar alignment: PaneHeader, GroupHeader, list_option, InfoBar, LocalClock | features/calendar/view/*, calendar/week.rs | U6 |
| U9 | Status/roadmap update with before/after screenshots, design + accessibility checklist | docs/project/status.md, roadmap.md | all |

Follow-up data issues (not implemented now): preview snippet, unread counts, folder kind, attachments, importance. A's table lists the layers each one touches.

## Changes after review

- 2026-11-13, message rows (#455): every row shows a 24-hour time. The day group header above carries the date, so the row no longer repeats it as a weekday or short date, which matches Wino. The option's accessible description names the day and the time, for example "Yesterday, 20:00", because the header is not part of the option. This overrides candidate A's row time rule.
- 2026-11-13, Calendar (#457): New event moves from the Calendar header into the navigation pane, above a mini month, because the owner's Wino Calendar reference puts them there. This ends the asymmetry that candidate A accepted, and it keeps the `calendar-new-event` id and its capability and sync gates. The header becomes a `PaneHeader` with the month title, Today, previous and next week, and Sync as icon controls. The week reads as seven day columns above 44rem of content width and as the grouped agenda below it. Both layouts fill the same one listbox in the flat event order of ADR 0042.
