# Candidate A: Fluent through the theme registry, one primitive per pattern

Thesis: the Fluent look comes from a **registered GPUI theme config**, not from runtime field patches, plus **six small app primitives** in `apps/desktop/src/components/`. Feature code never names a color, a radius, a Lucide icon, or a pill dimension. `Theme::change(mode)` reapplies the palette by construction, so system appearance changes need no extra code.

## Problem

The desktop app uses GPUI Kit defaults plus one runtime patch (`apply_appearance`, workspace.rs:148: `Theme::change` and then `Theme::update(|t| t.ring = t.primary)`). Every screen builds its own header, card, notice, and selection styling. Constraints that shape the design:

- `Theme::change` reloads `theme.light_theme` / `theme.dark_theme` (`gpui-component-0.9.1/src/theme/mod.rs:398`, `edit` at :307). Any patch made after `change` must be repeated on every change. A config installed as `light_theme`/`dark_theme` is applied automatically.
- `ThemeConfig` (schema.rs:40) can already express every role we need: `sidebar.*`, `group_box.*`, `list.active/hover/head`, `secondary.active`, `accent`, `ring`, `radius`, `radius.lg`. An unparsable color falls back silently (`apply_config`, schema.rs:709), so tests must pin the values.
- Spacing tokens are fixed (`spacing_tokens()` returns the default). We keep the GPUI rem scale.
- ADR 0024 requires a flat ListBox index, `message-option-{id}` ids, `message-row-{id}` selectors, and `aria_position_in_set`/`size_of_set` over messages only.
- The default asset bundle `gpui_kit::assets::Assets` embeds only 106 icons (`gpui-kit-assets-0.9.1/default-icons.txt`). Reply, Archive, Flag, Trash, MailOpen, FolderInput, and SquarePen are not in it. `AllAssets` (lib.rs:41) embeds the full Lucide set.
- `MAIL_SHELL_NAVIGATION_WIDTH` (mail/view.rs:55) copies the shell's `w(rems(16.))` (workspace/render.rs:25). The shell also sets min 14 and max 20, but those limits never apply.
- `SidebarMenuItem` (sidebar/menu.rs:263) has no `track_focus`, no debug selector, and no pill, and it uses `Role::TreeItem`. It cannot keep the `nav-*` focus and test contract.

## Visual spec

### Token layer

One file, `apps/desktop/src/components/theme/einfach-fluent.json`, holds a `ThemeSet` with "Einfach Fluent Light" and "Einfach Fluent Dark". It is the only place in the crate with raw colors. `components/theme.rs` loads it into `ThemeRegistry` once, installs both configs as `Theme.light_theme`/`dark_theme`, and calls `Theme::change(mode)`. `ring` moves into the JSON, so the `Theme::update` patch is deleted. Keys that are not listed inherit GPUI defaults through `apply_config`.

| Fluent role | JSON key | Light | Dark | Used by |
|---|---|---|---|---|
| Mica base (opaque) | `sidebar.background` | #F3F3F3 | #202020 | nav pane |
| Layer (content) | `background` | #F9F9F9 | #282828 | list, reader, pages |
| Card | `group_box.background` | #FFFFFF | #2F2F2F | SettingsCard, account cards |
| Card / divider stroke | `border`, `sidebar.border` | #E5E5E5 | #3B3B3B | hairlines |
| Text primary | `foreground`, `*.foreground` | #1B1B1B | #FFFFFF | body |
| Text secondary | `muted.foreground` | #5F5F5F | #C8C8C8 | captions, time |
| Subtle hover | `accent.background` | #EAEAEA | #323232 | ghost hover, menus |
| Selected nav fill | `secondary.active.background` | #E3E3E3 | #383838 | ghost `selected` (button.rs:1330) |
| Selected list row | `list.active.background` | #D6E3F0 | #304148 | message and agenda options |
| Row hover | `list.hover.background` | #F0F0F0 | #2F2F2F | options |
| Group band | `list.head.background` | #F3F3F3 | #202020 | GroupHeader |
| Accent | `primary.background` / `.hover` / `.active` | #005FB8 / #1A6FBF / #337FC6 | #60CDFF / #59BBE8 / #52A9D1 | New mail, pill, unread text |
| On accent | `primary.foreground` | #FFFFFF | #000000 | |
| Focus ring | `ring` | #005FB8 | #60CDFF | replaces runtime patch |
| Status icon colors | `info` / `warning` / `danger` / `success` | #005FB8 / #9D5D00 / #C42B1C / #0F7B0F | #60CDFF / #FCE100 / #FF99A4 / #6CCB5F | InfoBar |
| Popover | `popover.background` | #FCFCFC | #2C2C2C | menus |

Radius: `radius: 4` and `radius.lg: 8`. In `radius_tokens()` (mod.rs:585) that gives sm 2, md 4 (controls, nav items, rows), lg 8 (cards, InfoBar, popovers), and full (pill, avatar). `shadow` keeps its default so only overlays rise above the base. Font: platform UI font, base 16px. The Fluent type ramp maps to the existing rem helpers: Caption `text_xs`, Body `text_sm`, Body strong `text_sm().font_semibold()`, Subtitle `text_xl().font_semibold()`, page title `text_2xl().font_semibold()`.

Derived colors, such as InfoBar tints (`tone_color.opacity(0.12)`), come only from `components/theme.rs::status_colors`. Each geometry token is private to the primitive that uses it: pill 0.1875rem × 1rem (3×16px at base size, radius full, `primary`), nav item height 2.25rem, page column 40rem, card gap 0.25rem, row avatar 2rem.

## Usage (caller's view)

```rust
// app/workspace/render.rs: bottom destination switcher (nav-* ids and focus handles kept)
let (id, label, icon) = section.navigation_identity();          // icon: AppIcon
NavItem::icon_only(id, label, icon)
    .selected(self.content.selected_section() == section)
    .unavailable(self.navigation_disabled_reason(section, cx))   // Option<&'static str> -> tooltip + disabled
    .track_focus(self.navigation_focus(section))
    .on_click(cx.listener(move |w, _, _, cx| w.navigate_to(section, cx)))

// features/settings/view.rs
Page::new("settings-scroll-viewport", "Settings")
    .description("Choose how Einfach Mail displays its workspace.")
    .child(
        SettingsCard::new("settings-appearance-group", "Appearance")
            .icon(AppIcon::Appearance)
            .description("Follow the desktop theme or choose light or dark.")
            .trailing(render_appearance_options(appearance, on_change)),
    )
    .when_some(notice, |page, notice| {
        page.child(InfoBar::status("settings-appearance-notice", notice.message()).tone(StatusTone::Warning))
    })

// features/mail/view/message_reader.rs
let view = cx.entity().downgrade();
CommandBar::new("message-actions")
    .overflow(self.reader_uses_overflow(window))
    .command(Command::new("message-reply", "Reply", AppIcon::Reply)
        .disabled(reply_disabled)
        .on_invoke(command_handler(&view, MailView::open_reply_composer)))
    .command(Command::new("message-archive", archive_label, AppIcon::Archive).loading(archiving) /* … */)
    .command(Command::menu("message-move", "Move to…", AppIcon::Move, move_menu_builder))
    .command(Command::new("message-delete", delete_label, AppIcon::Delete) /* … */)
    .command(Command::new("message-read-state", read_label, read_icon).secondary() /* … */)
    .command(Command::new("message-follow-up-flag", flag_label, flag_icon).secondary() /* … */)

// features/mail/view/message_list.rs, inside v_virtual_list (flat index i)
let summary = MessageRowSummary::new(message, &self.mail_clock);
v_flex()
    .when(day.starts_group(), |item| item.child(GroupHeader::new(summary.day_label())))
    .child(
        list_option(format!("message-option-{id}"), SetPosition::new(i, len), selected, cx)
            .debug_selector(move || format!("message-row-{id}"))
            .aria_label(summary.accessible_label())       // unchanged "Name <addr>. Subject"
            .aria_description(summary.accessible_details()) // "Unread. Today 09:14. Flagged"
            .child(render_message_row(&summary, window, cx)),
    )
```

## Shape

```rust
// components/theme.rs (pub(crate))
const FLUENT_THEME_SET: &str = include_str!("theme/einfach-fluent.json");
/// The only appearance entry point. Idempotent: registers the set once, installs both
/// configs, then `Theme::change(mode)`. Replaces both lines of `Workspace::apply_appearance`.
pub(crate) fn apply_app_theme(mode: ThemeMode, window: &mut Window, cx: &mut App) { todo!() }
#[derive(Clone, Copy)] pub(crate) enum StatusTone { Informational, Success, Warning, Error }
/// Reads legacy component fields in exactly one place.
pub(crate) struct StatusColors { background: Hsla, border: Hsla, icon: Hsla }
pub(crate) fn status_colors(tone: StatusTone, cx: &App) -> StatusColors { todo!() }

// components/icon.rs: the product icon lexicon, one Lucide family
#[derive(Clone, Copy)]
pub(crate) enum AppIcon { Mail, Calendar, Accounts, Settings, NewMail, Folder, Reply, Archive,
    Move, Delete, MarkRead, MarkUnread, Flag, Unflag, Sync, More, Back, Appearance,
    Info, Warning, Error, NarrowList, WidenList }
impl IconNamed for AppIcon { fn path(self) -> SharedString { todo!() } } // -> gpui_kit::assets::IconName::X.path()

// components/nav_item.rs: replaces render_navigation_button and render_folder_button
#[derive(IntoElement)]
pub(crate) struct NavItem { id: SharedString, label: SharedString, icon: Option<AppIcon>,
    icon_only: bool, selected: bool, unavailable: Option<&'static str>,
    focus: Option<FocusHandle>, on_click: Option<ClickHandler> }
impl NavItem {
    pub(crate) fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self { todo!() }
    /// Accessible name = label; tooltip = unavailable reason or label.
    pub(crate) fn icon_only(id: &'static str, label: &'static str, icon: AppIcon) -> Self { todo!() }
    pub(crate) fn icon(self, icon: AppIcon) -> Self { todo!() }
    pub(crate) fn selected(self, selected: bool) -> Self { todo!() }
    pub(crate) fn unavailable(self, reason: Option<&'static str>) -> Self { todo!() }
    pub(crate) fn track_focus(self, focus: &FocusHandle) -> Self { todo!() }
    pub(crate) fn on_click(self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self { todo!() }
}
// render: div().relative().w_full()
//   .child(Button::new(id).ghost().selected(..).debug_selector(id).accessibility_label(label)
//          .child(icon slot).child(label div, font_semibold when selected))
//   .child(div().absolute().left_0().top_0().bottom_0().flex().items_center()
//          .child(pill when selected)) // the overlay has no listeners, so pointer hits reach the Button

// components/page.rs: replaces settings_content_frame, account render_header max_w_48,
// and the workspace's both-axes centering wrapper
#[derive(IntoElement)]
pub(crate) struct Page { scroll_id: ElementId, title: SharedString,
    description: Option<SharedString>, children: Vec<AnyElement> }
impl Page { pub(crate) fn new(scroll_id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self { todo!() }
            pub(crate) fn description(self, d: impl Into<SharedString>) -> Self { todo!() } }
impl ParentElement for Page { fn extend(&mut self, e: impl IntoIterator<Item = AnyElement>) { todo!() } }

// components/settings_card.rs: replaces appearance_group_frame, account_identity, render_permissions
#[derive(IntoElement)]
pub(crate) struct SettingsCard { id: ElementId, title: SharedString, icon: Option<AppIcon>,
    leading: Option<AnyElement>, description: Option<SharedString>,
    trailing: Option<AnyElement>, body: Vec<AnyElement> }
// Role::Group, aria_label = title. Trailing control wraps below the text at narrow widths.
// `leading` takes an Avatar for identity cards.

// components/info_bar.rs: replaces border_l_2 notices and ad-hoc Status/Alert divs
#[derive(IntoElement)]
pub(crate) struct InfoBar { id: ElementId, role: Role, tone: StatusTone,
    title: Option<SharedString>, message: SharedString, action: Option<Button> }
impl InfoBar { pub(crate) fn status(id: impl Into<ElementId>, m: impl Into<SharedString>) -> Self { todo!() }
               pub(crate) fn alert(id: impl Into<ElementId>, m: impl Into<SharedString>) -> Self { todo!() } }
// No close button: none of today's notices can be dismissed.

// components/command_bar.rs
pub(crate) type CommandHandler = Rc<dyn Fn(&mut Window, &mut App)>;
pub(crate) struct Command { id: &'static str, label: SharedString, icon: AppIcon,
    priority: CommandPriority, disabled: bool, loading: bool, kind: CommandKind }
enum CommandPriority { Primary, Secondary }   // Secondary moves to "More" in overflow mode
enum CommandKind { Invoke(CommandHandler), Menu(Rc<dyn Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu>) }
#[derive(IntoElement)]
pub(crate) struct CommandBar { id: ElementId, overflow: bool, commands: Vec<Command> }
// Expanded: ghost Button with icon and label for each command. Overflow: Secondary commands
// become PopupMenuItem::new(label).icon().disabled().on_click(...) behind an AppIcon::More
// button (accessibility_label "More actions"). Menu commands are always Primary.

// components/group_header.rs and components/list_option.rs
#[derive(IntoElement)] pub(crate) struct GroupHeader { label: SharedString }   // no role, band
#[derive(Clone, Copy)] pub(crate) struct SetPosition { index: usize, size: usize }
/// The ADR 0024 / ADR 0042 option contract in one place: ListBoxOption role, set position,
/// aria_selected, active descendant, list.active fill, list.hover, radius md.
pub(crate) fn list_option(id: impl Into<ElementId>, pos: SetPosition, selected: bool, cx: &App) -> Stateful<Div> { todo!() }

// components/pane_header.rs: replaces MailView::render_header (MAIL_HEADER_HEIGHT) and calendar render_title_bar
#[derive(IntoElement)]
pub(crate) struct PaneHeader { title: SharedString, badge: Option<SharedString> /* "Read-only" */,
    status: Option<AnyElement> /* keeps mail-sync-status */, actions: Vec<AnyElement> }

// components/local_time.rs: moved out of calendar/week.rs so mail and calendar share it
pub(crate) struct LocalClock { zone: TimeZone, today: civil::Date }
impl LocalClock { pub(crate) fn system() -> Self { todo!() } /* IANA zone or UTC fallback */ }

// components/shell.rs: one owner for shell geometry that features read
pub(crate) const NAVIGATION_PANE_WIDTH: Rems = rems(16.);

// features/mail/view/day_groups.rs: mail-owned
#[derive(Clone, Copy)] pub(super) struct MessageDay { date: civil::Date, starts_group: bool }
/// Called only from `MailView::set_messages`, the single assignment site (view.rs:306).
/// A group starts when the local date differs from the previous row, so unsorted input is still correct.
pub(super) fn message_days(messages: &[MailMessage], clock: &LocalClock) -> Rc<[MessageDay]> { todo!() }
/// Virtual-list sizes are derived from `message_days` and rem. They replace the uniform vector.
pub(super) fn row_sizes(days: &[MessageDay], rem: Pixels) -> Rc<Vec<Size<Pixels>>> { todo!() }
pub(super) fn day_label(date: civil::Date, today: civil::Date) -> SharedString { todo!() } // Today | Yesterday | "Sunday, October 25, 2026"
pub(super) struct MessageRowSummary { /* sender_name, sender_full, subject, time_label, unread, flagged, day_label */ }
```

Load-bearing decisions:
- The palette has one owner: the registered config. Theme state has no second writer, and appearance changes need no extra code (per *single source of truth*).
- Date headers sit inside virtual item *i*, above its option. Index, keyboard actions, `ListNavigation`, and `scroll_to_item(i)` stay unchanged. `MessageDay` is derived in `set_messages`, and row sizes are derived from it (per *derive instead of sync*). Labels such as Today and Yesterday use "now" at render time. Group boundaries depend only on dates and the time zone.
- `unavailable(Option<&'static str>)` makes a disabled control without a reason impossible to write (per *encode-lessons-in-structure*).
- `AppIcon` keeps feature code away from Lucide names. A command and its overflow menu item can't use different icons.
- Not done: no Sidebar component, no custom spacing scale, no translucency change, no Focused/Other tabs, no search, no hamburger, no settings sub-navigation, no breadcrumb trail (no sub-page exists today).

Interface depth: six primitives with 3–7 builder methods each. Behind them sit focus, tooltip and disabled policy, the pill overlay, overflow menu construction, the ListBoxOption ARIA contract, and severity-to-color derivation.

## Per-screen layout

**Shell nav pane** (`NAVIGATION_PANE_WIDTH` fixed 16rem, `sidebar` surface, hairline on the trailing edge), from top to bottom:
1. Account header: `Avatar::new().name(label)` at 2rem, label semibold, address caption. Keeps `active-account-identity` and "Active account: …".
2. Contextual command (Mail only): primary "New mail" button with `AppIcon::NewMail`, full width. It keeps id `new-mail-message`, shows "Resolve send" while a send is pending, and moves out of the `MailView` header into `MailFolderNavigation`, which calls `mail_view.update(|v, cx| v.open_composer(window, cx))`.
3. "Folders" caption, then one `NavItem::new("folder:{id}")` with `AppIcon::Folder` per folder. The icon slot is reserved for folder-kind icons. Region Role::Group "Mail folders", scrolls on its own.
4. Destination switcher above a top hairline: four `NavItem::icon_only` items spread evenly (`nav-mail`, `nav-calendar`, `nav-accounts`, `nav-settings`). This replaces the top Mail/Calendar buttons and the bordered Accounts and Settings blocks. The brand row is dropped because the system title bar already names the app.

`WindowOptions.window_min_size` becomes NAVIGATION_PANE_WIDTH + MESSAGE_LIST_MIN_WIDTH + handle reserve, so the pane never squeezes. `uses_compact_mail_layout` reads `components::shell::NAVIGATION_PANE_WIDTH`, and `MAIL_SHELL_NAVIGATION_WIDTH` is deleted. No rail mode in this effort.

**Message list header** (it replaces the "Mail" header): selected folder name (`text_xl` semibold), Read-only text, `mail-sync-status` caption, and icon buttons for Sync, Narrow list, and Widen list. Their `accessibility_label`s stay "Sync", "Narrow list", and "Widen list", with tooltips. **Rows** stay one fixed height (4.5rem, two lines):
`[Avatar 2rem] sender display name ……… time` / `subject ……… flag icon`.
Unread rows show the sender in semibold and the subject and time in semibold `primary`. Read rows use regular weight with `muted_foreground` for the time. Weight differs, not only color, and `aria_description` says "Unread". The time shows HH:MM for today, a weekday name within the last 7 days, and "Oct 25" (plus the year when it differs) for older mail. A group item is header height (1.75rem) plus row height.

**Reader**: the subject (`text_xl` semibold), sender line, and feedback notices (as `InfoBar`) sit below a `CommandBar` with a hairline under it. Overflow turns on when the reader width is under 40rem, computed in `MailView` from the window width, `NAVIGATION_PANE_WIDTH`, and the list width (or the full content width in compact mode). Delete becomes a neutral ghost button, because it moves the message to Deleted Items and can be undone. In compact mode a back button with `AppIcon::Back` leads the command bar.

**Accounts / Settings**: a `Page` with a 40rem centered column, padding `p_8`, title `text_2xl`, a description, and `SettingsCard` stacks with a 0.25rem gap between cards inside a group and `gap_6` between groups. Accounts: an identity card (Avatar leading, status caption, Sign out trailing), a permissions card (body rows), and two provider cards with Connect trailing. The EWS form sits in the Exchange card body. Below 40rem + 2×`p_8` the column shrinks and trailing controls wrap.

**Calendar**: a header like the list header (title "Calendar", week navigation, New event, a Sync icon button). Day headings use `GroupHeader`. Agenda rows use `list_option`, so they get the same selected fill, hover, and radius as messages. Status text uses `InfoBar`.

## Accessibility and test contract

Kept unchanged: Role::Navigation "Main navigation", "Mail folders", `nav-*` debug selectors and focus handles, `folder:{id}`, ListBox "Messages", `message-option-{id}`, `message-row-{id}`, option `aria_label` text, set position and size over messages, `mail-sync-status`, `new-mail-message`, `sync-mail`, `message-*` action selectors in expanded mode, `settings-appearance-group`, `settings-appearance-notice`, `account-scroll-viewport`, `settings-scroll-viewport`, and the radio roles and labels.

Tests that need deliberate updates:
- message_list.rs:545–560: `message_row_sizes[0].height` includes the header when item 0 starts a group. The option height stays 72px.
- Any test that clicks `message-read-state` or `message-follow-up-flag` at a reader width under 40rem: assert the More menu path, or widen the window.
- Tab order is now New mail, folders, Mail, Calendar, Accounts, Settings. Mail→Calendar stays adjacent, so `focus_calendar_after_mail` holds.
- appearance.rs contrast assertions extend to every pair in the token table. A new `fluent_theme_values_survive_mode_changes` test switches Light→Dark→System and asserts the exact `primary`, `ring`, `sidebar`, and `list_active` values (this catches a silent parse fallback). `danger` vs `background` ≥ 3:1 stays.
- New tests: pill bounds (3×16 at base rem, vertically centered in the selected item, absent when unselected); date header does not change `aria_position_in_set`; overflow menu has the same enabled state as the buttons; icon-only nav names come from `find_by_label("Settings")`.

## ADR draft outline: 0066 "Fluent visual language and destination switcher"

Status Proposed; amends ADR 0059. **Context**: the user wants Wino-style fidelity. The 0059 bullet bans an accent marker. The design guide forbids one-sided edge bars. **Decision**: (1) The selected nav item uses a filled rounded surface plus a short, centered, fully rounded accent pill *inside* the item silhouette. This differs from the banned full-height one-sided border: it doesn't touch the item corners or add a second edge. Message and agenda rows use fill only. (2) A bottom icon switcher for Mail, Calendar, Accounts, and Settings sits inside the one contextual pane (not a fourth column), with tooltips and accessible names. (3) The Fluent palette lives in a registered ThemeSet, with radius 4/8 and base font 16. (4) The full Lucide asset bundle (`AllAssets`) is used through `AppIcon`. (5) Compose moves to the pane as "New mail". Delete becomes neutral. **Alternatives**: fill-only (0059 as is), separate app rail, gpui-component Sidebar, runtime `Theme::update` patches. **Consequences**: test updates, binary size, and the data follow-ups.

## Verification lever

These env vars exist only in `ui-preview` builds and are parsed in `bootstrap.rs` next to `PREVIEW_SCENARIO_ENV`:
- `EINFACH_MAIL_UI_PREVIEW_DESTINATION=accounts|mail|mail-reader|calendar|settings`. `Workspace::with_start_destination` (cfg ui-preview) stores it. The existing `_account_view_observation` consumes it once `navigation_disabled_reason` is `None`. `mail-reader` then runs the existing select-first and open actions.
- `EINFACH_MAIL_UI_PREVIEW_APPEARANCE=light|dark` seeds the temporary preference file.
- `EINFACH_MAIL_UI_PREVIEW_WINDOW_SIZE=1280x800` sets `window_bounds` for compact screenshots (for example 900x700).

```sh
for d in mail mail-reader calendar accounts settings; do for a in light dark; do
  EINFACH_MAIL_UI_PREVIEW_DESTINATION=$d EINFACH_MAIL_UI_PREVIEW_APPEARANCE=$a OPENSSL_NO_VENDOR=1 \
    cargo run --locked -p einfach-mail-desktop --features ui-preview --bin einfach-mail & sleep 6
  grim -g "$(hyprctl clients -j | jq -r '.[]|select(.title=="Einfach Mail")|"\(.at[0]),\(.at[1]) \(.size[0])x\(.size[1])"')" "$d-$a.png"
  kill %1; done; done
```

Headless checks: `cargo test -p einfach-mail-desktop` (contrast, pill bounds, row sizes, ids), plus the README checks (fmt, clippy, depgraph unchanged, `cargo deny check bans`).

## Decomposition

1. **ADR 0066 + dependency-review entry for `AllAssets`** (docs). *Blocking.*
2. **Preview lever** (bootstrap.rs, preview.rs, workspace.rs builder). *Blocking for screenshot evidence.*
3. **Token layer**: JSON, `components/theme.rs`, `components/icon.rs`, `apply_appearance` → `apply_app_theme`, `with_assets(AllAssets)`, contrast and value tests. *Blocking for 4–8.*
4. **Shell**: `NavItem`, `components/shell.rs`, switcher, account header, folder items, New mail move, window min size (workspace/render.rs, state.rs, mail/view/folder_navigation.rs, mail/view/render.rs header removal).
5. **Settings**: `Page`, `SettingsCard`, `InfoBar` (settings/view.rs only). *Parallel with 4.*
6. **Mail list**: `local_time`, `day_groups`, `GroupHeader`, `list_option`, `PaneHeader`, row anatomy (mail/view.rs, message_list.rs, render.rs). After 4.
7. **Reader command bar**: `CommandBar` (message_reader.rs, message_actions.rs). After 4, *parallel with 6* (disjoint files; the overflow width function lives in message_reader.rs).
8. **Accounts**: reuse Page/Card/InfoBar (account/render.rs, workspace `render_account_workspace_content`). After 4 and 5.
9. **Calendar**: reuse list header, GroupHeader, list_option, InfoBar; calendar/week.rs uses `LocalClock`. After 6.
10. **Status and roadmap update** with screenshots from step 2.

Each primitive lands with its first consumer, so the build never carries dead code.

## Follow-up data issues (not implemented now)

| Data | Provider | Domain / application | Persistence | UI |
|---|---|---|---|---|
| Preview snippet | Graph `bodyPreview`; EWS `Preview` where the server supports it | `MessagePreview` newtype (plain text, length-capped) on metadata | cache column + migration | third row line; row height 5.5rem |
| Unread counts | Graph `unreadItemCount`; EWS `UnreadCount` | `MailFolder::unread_count`; workflow adjusts on read-state change | folder column | `NavItem` trailing `Badge::count` |
| Folder kind | Graph well-known aliases resolved to ids; EWS `DistinguishedFolderId` | `MailFolderKind` (extends `WellKnownMailFolder`) | folder column | `AppIcon` per kind, pinned order |
| Attachments | Graph `hasAttachments`; EWS `HasAttachments` | `has_attachments: bool` | column | paperclip in row lane 2 |
| Importance | Graph `importance`; EWS `Importance` | `Importance` enum | column | icon in row and reader |

UI-only follow-up (the data already exists): folder nesting by `MailFolder::parent()` depth, once folder ordering is guaranteed to be a tree order.

## Synthesis decision

*(Filled by arena.)*

## Tradeoffs accepted

- We accept a larger binary (full Lucide set) in exchange for one icon family with no custom asset plumbing.
- We accept that the pill departs from the design guide's no-edge-marker rule in exchange for Fluent fidelity, and we record it in ADR 0066.
- We accept a fixed 16rem nav (no rail) in exchange for not shipping a rail of identical folder icons before folder kinds exist.
- We accept that `StatusColors` reads legacy fields in exchange for keeping that coupling in one function.
- We accept the asymmetry of New mail in the pane while New event stays in the Calendar header, until the calendar needs a contextual pane.

## Alternatives considered

- **Runtime `Theme::update` patches after each `change`** (today's pattern, extended): it gives two writers of theme state, every new role needs a repeat call, and it is easy to forget on a new appearance path. Rejected because it leaks palette policy into the shell.
- **gpui-component `Sidebar`/`SidebarMenuItem`**: it hides less than it looks. It lacks focus handles, debug selectors, and the pill, and it exposes TreeItem semantics. Adopting it would break the `nav-*` contract.
- **Date headers as separate virtual items with an index map**: this needs two index spaces, `ListNavigation` must skip headers, and `scroll_to_item` must translate. That exposes complexity to every keyboard action. Rejected for headers inside the item.
- **App-owned `SemanticThemeTokens` with a Fluent spacing scale**: GPUI does not keep custom spacing, so every primitive would need a context handle. The rem scale already fits Fluent's 4px grid.
- **gpui-component `Alert` for InfoBar**: it hard-codes `Role::Alert` and px padding (alert.rs:179-192), which would turn polite Status notices into assertive alerts.

## Open questions and risks

- Should the base font drop to 14px to match Windows density? That would change every rem layout and the zoom tests. This design says no.
- Is a fixed pane acceptable below about 900px wide, or does ADR 0066 need a rail mode with folder kinds?
- Is a 12- or 24-hour clock acceptable without locale support? The sketch uses 24-hour.
- Is the `AllAssets` binary-size increase acceptable, or should we curate a subset with an app `AssetSource`?
- Should Delete lose `danger` styling? It is reversible, but this changes current behavior.
- Risk: a silent color fallback in `apply_config`. The exact-value tests mitigate it.

## Next implementation step

Write `einfach-fluent.json` and `components/theme.rs::apply_app_theme`, and replace `Workspace::apply_appearance`'s two lines, guarded by the exact-value and contrast test.
