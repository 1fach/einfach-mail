# Candidate B: theme-JSON token layer plus thin app-owned Fluent primitives

## Problem

Make Einfach Mail read as a Windows 11 / Fluent app using only today's data (subject, sender, `received_at`, read state, flag). The user fixed these constraints: no new domain/provider/persistence fields, only working controls, system title bar, ADR 0059 amended to allow a filled selection surface plus a short accent pill, and a bottom icon switcher. Today the only theme customization is `Theme::update(|t| t.ring = t.primary)` after each `Theme::change` (`workspace.rs:148`). Cards, notices and headers are ad-hoc `div`s in account/settings/calendar/mail. Verified blockers for "just use library parts": `SidebarMenuItem` hard-codes `h_7`, `p_2`, a fill-only active state and no pill (`gpui-component-0.9.1/src/sidebar/menu.rs`, `render`). `setting::Settings` owns its own sidebar, search and pages (`setting/settings.rs`). Neither fits a single-column Settings page or a nav pane that mixes folders with a bottom switcher. The default icon bundle has 106 icons and no Mail/Flag/Archive/Reply/Trash (`gpui-kit-assets-0.9.1/default-icons.txt`).

## Visual spec

**Where it lives.** One `ThemeSet` JSON, `apps/desktop/src/theme/fluent.json`, with "Einfach Fluent Light" and "Einfach Fluent Dark". `theme::install(cx)` runs in `initialize_application` (bootstrap.rs:~225) before the window opens. It parses `ThemeSet` (pub in `theme/schema.rs:25`), derives `ring = primary` on each `ThemeConfig` (`ThemeConfigColors` fields are pub, `schema.rs:119,131`), then runs `Theme::update(cx, |t| { t.light_theme = Rc::new(light); t.dark_theme = Rc::new(dark); })`.

**Survival.** `Theme::change` reloads `theme.light_theme`/`dark_theme` via `apply_config` (`theme/mod.rs:300-345`; its doc says swapped configs are applied on `change`). The existing `apply_appearance` and system-appearance path therefore re-apply Fluent for free. The `ring` override disappears. Slots the JSON omits fall back to `ThemeColor::light()/dark()` defaults (`apply_config`, schema.rs:1100).

Raw colors exist only in `fluent.json`. A unit test greps `apps/desktop/src` for `rgb(`, `rgba(`, `hsla(` and `#[0-9a-fA-F]{6}` outside `theme/`.

**Roles.** The ThemeConfig schema has no custom keys, so each Fluent role is a documented mapping onto an existing slot. `Fluent` (below) is the only place that knows the mapping.

| Role | Slot | Light | Dark |
|---|---|---|---|
| Nav layer (Mica stand-in, opaque) | `sidebar.background` | #F0F0F0 | #202020 |
| Canvas (list pane, pages) | `background`, `list.background` | #F9F9F9 | #272727 |
| Card / reader layer | `group_box.background`, `popover.background` | #FFFFFF | #2D2D2D |
| Hairline | `border`, `sidebar.border`, `input.border` | #E5E5E5 | #3A3A3A |
| Date band | `list.head.background` | #F0F0F0 | #202020 |
| Hover fill (alpha, state only) | `list.hover.background`, `secondary.hover.background` | #0000000D | #FFFFFF0F |
| Selected nav fill | `sidebar.accent.background`, `button.active.background` | #00000014 | #FFFFFF14 |
| Selected message row | `list.active.background` | #005FB81A | #4CC2FF26 |
| Accent (fill, pill, link) | `primary.background`, `sidebar.primary.background`, `link`, `ring` | #005FB8 | #4CC2FF |
| On accent | `primary.foreground` | #FFFFFF | #000000 |
| Text primary / secondary | `foreground` / `muted.foreground` | #1B1B1B / #5F5F5F | #FFFFFF / #C5C5C5 |
| Status | `danger`, `success`, `warning`, `info` | #C42B1C, #0F7B0F, #9D5D00, #005FB8 | #FF99A4, #6CCB5F, #FCE100, #4CC2FF |

Hex values are the proposed starting palette. The headless contrast test (below) is the acceptance gate, not the table.

**Radius tiers.** `radius: 4` and `radius.lg: 8`. `Theme::radius_tokens()` (`theme/mod.rs:585`) then yields sm 2 (pill), md 4 (controls, nav items, rows, command items), lg 8 (cards, info bars) and xl 8. Feature code uses `cx.theme().radius_tokens()`, never numbers. `window.radius` stays 0 because Linux keeps the system title bar.

**Not set.** `font.size` stays at 16, so `text_sm` is the Fluent 14 px body and rems keep scaling. The font stays `.SystemUIFont`. `shadow` is untouched (see open questions).

**Selection pill.** It is a metric, not a color. `metrics.rs` defines `PILL_LONG = rems(1.0)` and `PILL_THICK = rems(0.1875)`, filled with the accent role, radius sm, vertical in the nav list and horizontal under the switcher icon.

**Surface layering.** Layers are L0 nav, L1 canvas and L2 card/reader. Contrast between layers plus hairlines carries hierarchy. There are no shadows on cards and no nested cards. Translucent nav (#274) keeps working: `NavigationSurfaceAppearance` still wraps `fluent().layer_nav()` as a `Background`.

## Usage first

```rust
// 1. Nav pane (workspace/render.rs): folders and switcher share one primitive
NavItem::new(format!("folder:{id}"), folder.display_name())
    .icon(IconName::FolderClosed)          // generic until folder kind exists
    .selected(selected).disabled(busy)
    .on_click(move |_, _, cx| select(reference.clone(), cx))
// bottom switcher
NavSwitcher::new("workspace-switcher", Role::Navigation, "App destinations")
    .item(SwitcherItem::new("nav-mail", "Mail", IconName::Inbox).selected(on_mail).track_focus(h))
    .item(SwitcherItem::new("nav-settings", "Settings", IconName::Settings)…)

// 2. Settings card + info bar (features/settings/view.rs)
PageFrame::new("settings-scroll-viewport")
    .title(Breadcrumb::new().child(BreadcrumbItem::new("Settings")))
    .subtitle("Choose how Einfach Mail displays its workspace.")
    .child(SettingsCard::new("settings-appearance-group", "Appearance")
        .row(SettingsRow::new("Theme").icon(IconName::Palette)
             .description("System follows your desktop.").trailing(radios)))
    .children(notice.map(|n| InfoBar::new("settings-appearance-notice", n.severity())
        .message(n.copy()).on_close(dismiss)))

// 3. Message row (mail/view/message_list.rs): data in, fixed anatomy out
MessageRow::new(&message, now_clock)       // sender name, subject, time, unread, flag
    .selected(selected).group_header(starts_group.as_deref())
```

`MessageRow` and the date-group start vector stay private to the mail feature. Everything else is `components::fluent` and `pub(crate)`.

## Type/module sketch

```
apps/desktop/src/theme.rs            // pub(crate) install(), Fluent, FluentTheme; ADR 0067
apps/desktop/src/theme/fluent.json   // the only raw colors
apps/desktop/src/assets.rs           // icon_assets!(AppIcons,[Mail,MailOpen,Archive,Reply,Flag,Trash2,FolderInput]) + AssetSource composing Assets
apps/desktop/src/components/fluent.rs + fluent/{metrics,nav_item,nav_switcher,page,settings_card,info_bar,command_bar,view_header}.rs
apps/desktop/src/components/local_time.rs   // system tz + UTC fallback, extracted from calendar/week.rs:41-47
```

```rust
// theme.rs
pub(crate) fn install(cx: &mut App) { todo!("parse fluent.json; ring=primary; swap light_theme/dark_theme") }
#[derive(Clone, Copy)] pub(crate) struct Fluent { /* private Hsla roles */ }
impl Fluent {
    pub(crate) fn layer_nav(self) -> Background;   // sidebar token (keeps gradients/opacity path)
    pub(crate) fn canvas(self) -> Hsla;  pub(crate) fn card(self) -> Hsla;
    pub(crate) fn hairline(self) -> Hsla; pub(crate) fn date_band(self) -> Hsla;
    pub(crate) fn fill_hover(self) -> Hsla; pub(crate) fn fill_selected(self) -> Hsla;
    pub(crate) fn row_selected(self) -> Hsla;
    pub(crate) fn accent(self) -> Hsla; pub(crate) fn text_secondary(self) -> Hsla;
    pub(crate) fn status(self, s: Severity) -> Hsla;
}
pub(crate) trait FluentTheme { fn fluent(&self) -> Fluent; }
impl FluentTheme for App { fn fluent(&self) -> Fluent { todo!("read cx.theme()") } }

// components/fluent/nav_item.rs  (composes Button::ghost + absolute pill; keeps focus/disabled/tooltip)
#[derive(IntoElement)] pub(crate) struct NavItem { .. }
impl NavItem {
    pub(crate) fn new(id: impl Into<String>, label: impl Into<SharedString>) -> Self;
    pub(crate) fn icon(self, i: IconName) -> Self;
    pub(crate) fn suffix(self, e: impl IntoElement) -> Self;   // reserved lane for count Badge (follow-up)
    pub(crate) fn selected(self, b: bool) -> Self; pub(crate) fn disabled(self, b: bool) -> Self;
    pub(crate) fn on_click(self, f: impl Fn(&ClickEvent,&mut Window,&mut App)+'static) -> Self;
}
// nav_switcher.rs: icon-only items, tooltip == aria label, horizontal pill under selected
pub(crate) struct NavSwitcher; pub(crate) struct SwitcherItem;
// page.rs: scroll viewport + centered max_w(CONTENT_MAX_WIDTH) column + breadcrumb title
pub(crate) struct PageFrame; // .title(Breadcrumb) .subtitle() .child()
// settings_card.rs
pub(crate) struct SettingsCard; pub(crate) struct SettingsRow; // icon lane, title, description, trailing lane
// info_bar.rs
pub(crate) enum Severity { Info, Success, Warning, Error }     // Warning|Error => Role::Alert, else Role::Status
pub(crate) struct InfoBar;                                      // icon + text + optional on_close (X button)
// command_bar.rs
pub(crate) struct CommandItem { id, label, icon, danger, disabled: Option<SharedString>, priority: u8, on_click }
pub(crate) struct CommandBar;    // .available_width(Pixels): inline by priority, rest in DropdownMenu "More"
fn visible_count(avail: Pixels, rem: Pixels, items: &[CommandItem]) -> usize { todo!("pure, unit-tested") }
// view_header.rs: title, optional tag, status line, trailing actions (Mail list + Calendar)
pub(crate) struct ViewHeader;
```

**Replaced ad-hoc code** (one way each; delete in the same PR):
- Nav `Button` rows (`render_navigation_button`, `render_folder_button`, Accounts/Settings bordered buttons) become `NavItem`/`SwitcherItem`.
- `settings-appearance-group` frame, `account_identity` and `render_permissions` cards become `SettingsCard`.
- Notice divs in settings, account, calendar and mail (`border_l_2` destructive etc.) become `InfoBar`.
- Mail and calendar headers become `ViewHeader`.
- The reader's text button row becomes `CommandBar`.
- `MAIL_SHELL_NAVIGATION_WIDTH` becomes `metrics::NAV_WIDTH`.

**Interface depth.** `NavItem` hides focus, disabled, pill, fill, lane geometry and tooltips behind five setters. `InfoBar` hides severity-to-role/icon/color/aria. `PageFrame` hides centering, scrolling and breadcrumb semantics. `Fluent` hides the role-to-slot mapping, so a palette change touches only `fluent.json` and `theme.rs`.

## Per-screen layout

- **Shell nav (L0, fixed `NAV_WIDTH` 16rem).** From the top: account header (`Avatar::new().name(label)`, label, address; keeps `active-account-identity` Group). Then, in Mail only, a primary "New mail" button owned by `MailFolderNavigation` (which already observes `MailView`; same capability gate, "Resolve send" state, id `new-mail-message`). Then the folder `NavItem` list (one generic folder icon, suffix lane empty). Then a flex spacer and `NavSwitcher` (Mail, Calendar, Accounts, Settings). The top Mail/Calendar buttons and the bordered Accounts/Settings buttons go away. Calendar, Accounts and Settings show header plus empty body plus switcher; the shell stays stable per ADR 0059.
- **Compact widths and nav coupling.** `uses_compact_mail_layout` reads `metrics::NAV_WIDTH`, the same const the nav renders with, so the two cannot drift. The nav no longer uses min/max (it was `w 16 min 14 max 20`). Below the threshold, list and reader swap as today ("Back to messages"). Collapsing the nav to an icon rail is explicitly not done here.
- **Message list (L1).** `ViewHeader` shows the selected folder name, the sync status line and a ghost Sync button, with the Read-only tag. Date groups are a parallel `Vec<Option<DateGroup>>` over the flat `messages`, computed once in `set_messages`. Only the first message of a group carries `Some`. Each virtual-list item is a wrapper `div` (id `message-slot-{id}`, no role) containing an optional `DateGroupHeader` and the existing option `div`. `message_row_sizes` gives first-of-group items `MESSAGE_ROW_HEIGHT + GROUP_HEADER_HEIGHT`. The listbox index, `aria_position_in_set`, `aria_size_of_set` and ADR 0024 actions all stay on `messages`, so no index mapping exists. Headers are `Role::GenericContainer`; the option's aria label gains the time ("sender. subject. 14:05").
- **Row anatomy.** Unread lane (0.5rem accent dot), `Avatar` small (initials, deterministic OkLCH hue, theme-aware: `avatar/avatar.rs` `IdentityColor`), a text column (sender display name only; subject), and a fixed trailing lane (time, flag icon). Unread means semibold plus the accent dot plus accent sender text, never color alone. Time is HH:MM today, weekday this week, otherwise a short date. Selected rows get `row_selected` fill at radius md, with no pill (emphasis budget). The row wrapper stays `.h(MESSAGE_ROW_HEIGHT)`, with focus ring on the listbox.
- **Reader (L2).** Subject as title, sender line, then `CommandBar` (existing, capability-gated actions only): Reply, Delete (danger), Read/Unread, Flag, Archive, Move to…. Inline by priority; the rest go in a `DropdownMenu` "More" that appears only when `visible_count` < item count. The reader width comes from a new `MailView::reader_width(window)` beside `uses_compact_mail_layout`. Icons Reply/Flag/Archive/Trash2/Mail come from `assets.rs`.
- **Accounts and Settings.** `PageFrame` (canvas, `CONTENT_MAX_WIDTH` ≈ 46rem, 2rem gutters), breadcrumb title ("Settings"; "Accounts" with a second crumb for account details), subtitle, `SettingsCard`s (L2, hairline, radius lg; rows with icon lane, title/description, trailing lane; hairline row dividers), `InfoBar`s above cards. The workspace wrapper stops centering Accounts on both axes (top-aligned column).
- **Calendar.** `ViewHeader` replaces its header. Agenda rows use the same hover/selected fills and radius md as message rows. The grid is otherwise unchanged. ADR 0042 listbox behavior untouched.

## Accessibility and test contract

**Kept verbatim.**
- Ids and selectors: `nav-mail`, `nav-calendar`, `nav-accounts`, `nav-settings`, `folder:{id}`, `new-mail-message`, `message-option-{id}`, `message-row-{id}`, `settings-appearance-group`, `settings-appearance-notice`, `active-account-identity`, `mail-sync-status`, `mail-reader-back`.
- Roles and labels: `Role::Navigation` "Main navigation", Group "Mail folders", ListBox "Messages" with ListBoxOption position/size over the flat index, Status/Alert roles on notices, RadioGroup labels.
- The switcher items are icon-only, so each gets `aria_label` plus a tooltip (design guide: icon-only needs names). The tooltip text is the label.

**Deliberate test updates.**
- Option aria labels gain the time.
- Tests that locate `workspace-feature-navigation`, `workspace-account-navigation` and `workspace-settings-navigation` move to `workspace-switcher`.
- Row-size assertions include group headers.
- `appearance.rs` contrast tests add the new role pairs and read colors from `Fluent`.
- The workspace tests that open Accounts and expect centered layout change to top-aligned.

## ADR draft outline

- **ADR 0066, Fluent navigation (amends 0059):** status Accepted, "Amends 0059 decision bullets 2 and 3". Context: the Wino/Windows Mail references. Decision: a filled rounded selection surface plus a 3x16 accent pill is allowed on navigation destinations and folders only. The pill is one-per-list and never the only marker (fill and `aria_selected` remain). The bottom icon switcher for Mail/Calendar/Accounts/Settings has tooltips and accessible names. "New mail" lives in the mail folder section. The one-sided-stripe prohibition otherwise stands for lists and tabs. Consequences: the design guide's no-leading-bar rule is overridden for the pill, and nav is stable across destinations. Alternatives: four-column rail (rejected in 0059), top tabs.
- **ADR 0067, Fluent visual language and theme layer:** a ThemeSet JSON is the only color source; role mapping table; radius tiers 4/8; opaque layers L0-L2; alpha only for hover/selected fills; a library-first review (`dependency-review.md` rows for Sidebar, `setting`, Avatar, Breadcrumb, `icon_assets!`); Segoe is not bundled.

## Verification lever

- **Preview start.** Add `PreviewStart { destination, appearance }` parsed in `app/preview.rs` from `EINFACH_MAIL_UI_PREVIEW_DESTINATION=mail|calendar|accounts|settings` and `EINFACH_MAIL_UI_PREVIEW_APPEARANCE=light|dark|system`. It is passed through `ApplicationSetup` (cfg `ui-preview`) and applied by `Workspace::with_preview_start` (calls `navigate_to`, seeds the temp `preferences.json`). `scripts/preview-shots.sh` loops destination x appearance x two window widths (wide, compact via `hyprctl resizeactive`) and runs `grim`. The mail case needs the seeded `preview-message-1` selected, so `PreviewStart` also selects the first message.
- **Headless.** (1) `theme::tests`: parse `fluent.json`, assert both ThemeConfigs resolve. Check WCAG 4.5 for text/secondary on each layer and selected-row fill composited on canvas. Check 3:1 for accent and ring on layers, pill on nav, and status icons on card. (2) The no-raw-color source scan. (3) `gpui_kit::test` with `debug_bounds`: avatar x equal across rows, trailing lane x equal, the pill's bounds inside the selected item, `simulate_resize` crossing `NAV_WIDTH + list + reader` flips compact exactly at the threshold, `visible_count` unit tests. (4) Theme round-trip: `Theme::change(Dark)` then `Light` keeps `fluent().accent()` equal to the JSON value.

## Decomposition

1. **[Blocking] ADR 0066/0067 and theme layer** (`theme.rs`, `fluent.json`, `assets.rs`, bootstrap install, remove ring override, contrast and no-raw-color tests). Docs-only ADR PR may run in parallel with the code PR.
2. **[Parallel with 1] Preview start env and screenshot script.** Touches only `bootstrap.rs`, `preview.rs`, `workspace.rs` (`with_preview_start`) and scripts. Land first for before/after shots.
3. **After 1, then 3a-3c in parallel (disjoint files):**
   - 3a Shell nav: `NavItem`, `NavSwitcher`, account header, `MailFolderNavigation`, Compose move (`workspace/render.rs` nav functions, `folder_navigation.rs`, `render.rs` header button).
   - 3b Pages: `PageFrame`, `SettingsCard`, `InfoBar` plus settings and account migration (`features/settings/*`, `features/account/render.rs`, the workspace content wrapper function).
   - 3c List: `local_time.rs`, date groups, row anatomy, row sizes (`message_list.rs`, `view.rs`).
4. **After 3a:** `ViewHeader`, `CommandBar`, reader migration (`render.rs`, `message_reader.rs`, `message_actions.rs`).
5. **After 4:** Calendar alignment (`calendar/view/render.rs`).
6. **Final:** delete leftover ad-hoc notices, run the design and accessibility checklists, update `status.md` and `roadmap.md`, and add the dependency-review rows.

3a and 3b both touch `workspace/render.rs` in different functions, so a trivial rebase is expected.

## Follow-up data issues (not implemented now)

| Issue | Layers touched |
|---|---|
| Preview snippet | provider (Graph `bodyPreview`, bounded, control chars stripped, untrusted); domain `MessageMetadata::preview`; SQLCipher column plus migration; list DTO; UI row line 3 (+ row height token) |
| Per-folder unread counts | provider (`unreadItemCount`); domain `MailFolder`; cache plus optimistic update on read-state writes; UI `NavItem::suffix` with `Badge::count` |
| Folder kind | extend `WellKnownMailFolder` (`application/src/mail/provider.rs`, today Archive/DeletedItems) with Inbox/Sent/Drafts/Junk; provider resolves per folder; cache; UI icon map in the folder list |
| Attachment flag | provider (`hasAttachments`); domain; cache; row trailing lane icon (`Paperclip` via `icon_assets!`) |
| Importance | provider (`importance`); domain enum; cache; row trailing lane (high only) |

## Tradeoffs accepted

- We accept role-to-slot aliasing (`sidebar.primary.background` is the pill) in exchange for zero fork and free survival across `Theme::change`. The cost is that slot names mislead, so `Fluent` documents the mapping and is the sole reader.
- We accept an empty nav body on Calendar, Accounts and Settings in exchange for a stable shell.
- We accept alpha fills for hover/selected (not strictly opaque), because Fluent defines them that way and they stay correct on both layers.
- We accept a fixed 16rem nav (no collapse rail) to keep this effort to restyling.
- We accept a generic folder icon until folder kind lands.

## Alternatives considered

- **Adopt gpui-component `Sidebar` + `setting::Settings` wholesale.** This is a shallow win: more library, but the `SidebarMenuItem` pill, switcher and fixed `h_7` cannot be customized without CSS-style overrides (`refine_style` only), and `Settings` imposes its own navigation and search. It loses.
- **Code-level `Theme::update` overrides after each `change`** (today's pattern, extended). This is simple but drifts: every `change` site must remember to re-apply, and there is no schema validation. It loses to the one-time config swap.
- **Semantic-token config (`apply_semantic_config_str`).** It covers fewer slots (no sidebar/list) and has no per-mode persistence. It loses.
- **Date headers as separate list items with an index map.** This breaks the flat ListBox index (ADR 0024) and needs mapping in four handlers. It loses to header-in-wrapper.

## Open questions and risks

- Does ghost-Button selected/hover (`button.rs:1265,1330`) actually read `button.active.background`/`secondary.hover.background` as I mapped? Verify with the first screenshot; fall back to a bespoke NavItem surface if not.
- Do we keep `shadow` default (popovers keep elevation) or set false for flat controls? Needs a screenshot decision.
- 12 h or 24 h time and English-only date strings: is a fixed English format acceptable until localization?
- Is an app-level `icon_assets!` source composed with `Assets` acceptable under ADR 0061 and the dependency policy (it is already in the lockfile)?
- Is the accent #005FB8 fixed, or should it follow the system accent later (no portal API in GPUI)?
- Is the nav acceptable with nothing but header and switcher on Calendar/Accounts/Settings?

## Next implementation step

Write `fluent.json` and `theme::install`, plus the contrast unit test, then screenshot the existing UI in both modes through the preview start env to calibrate the palette.
