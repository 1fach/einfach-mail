# Grounding: Windows 11 / Fluent restyle of the Einfach Mail desktop UI

Repo: /home/frenico/Documents/development/hey-mail (Rust, GPUI via `gpui-kit` 0.9.1 = facade over `gpui-component` 0.9.1). Linux-first.

## User goal

The user wants Einfach Mail to feel like a Windows 11 app, using Wino Mail (`repos/wino-mail/`, GPL-3.0, inspection only, never copy code or assets) and the Windows Mail client as references. Reference screenshots show:
- left pane: hamburger + title, "New Mail" command, account identity with avatar, folder list with icons and unread count badges, a small accent pill marking the selected item on a filled rounded surface, and a bottom icon switcher (Mail, Calendar, People, To Do, Settings);
- message list: header row (select-all, Focused/Other, filter), date group headers ("Today", "Yesterday", "Sunday, October 25, 2026") on a muted band, two/three-line rows: avatar, sender (bold/accent when unread), subject, preview, time right-aligned, flag icon, unread dot;
- reader/composer: tab-like command bar (Format/Insert/Options), icon buttons, primary "Send" button;
- settings: grouped left nav with expandable sections, centered max-width column, breadcrumb title ("Manage Accounts > x details"), subtitle, segmented tabs, InfoBar notices with info icon and close button, "settings card" rows (icon, title, description, trailing control: toggle with On/Off label, chevron, text value, expander).
- Overall: light layered surfaces (Mica-like), 4px control radius / 8px card radius, hairline borders, generous whitespace, accent blue.

## Decisions already made by the user (constraints)

1. **Visual first, data later.** Restyle with data that exists today. Do NOT add provider/domain/persistence fields in this effort. Plan (as separate follow-up issues + ADR) the data additions: message preview snippet, per-folder unread counts, well-known folder kind (for Inbox/Sent/Drafts/Junk icons), attachment flag, importance.
2. **Amend ADR 0059 for Fluent navigation.** Allow a filled rounded selection surface plus a short accent pill indicator, and a bottom icon switcher for app destinations (Mail, Calendar, Accounts, Settings) with tooltips + accessible names. Record in a new ADR (next number after 0065).
3. Show only controls that work (AGENTS.md). No Focused/Other, People, To Do, search, select-all, rich-text toolbar, hamburger that does nothing, or "New Mail" unless wired to the existing composer (Compose exists in MailView header: it may move to the nav pane).
4. Keep: system title bar on Linux, one active account (ADR 0060), opaque content panes, translucency stays opt-in (#274), keyboard paths and accessibility (below).

## Design rules (from .agents/skills/gpui-kit-design-guides/references/design-guides.md, read it in full)

- Tokens before values: no raw hex/rgb in feature code. A Fluent palette must live in the product's theme/token layer.
- Platform UI font (do not bundle Segoe). One icon family. Icon-only buttons need accessible names and tooltips.
- Flat base surfaces; no shadow on every card; avoid nested cards. Concentric radii. Finite radius scale.
- Badges only for counts/states. Button for in-app commands, Link only for URLs.
- Run the Design review + Accessibility checklists at the end.

## Current code (from three explorers; file:line approximate)

### Shell (apps/desktop/src/app/)
- `Workspace<P>` workspace.rs:22-42 owns account view, `AccountSession`, nav focus handles, `content: WorkspaceContent`, appearance.
- `WorkspaceSection {Mail, Calendar, Accounts, Settings}` and `navigation_identity()` (id, label, IconName: Inbox/Calendar/User/Settings) in workspace/state.rs:14-56. `WorkspaceContent {Accounts, Settings, Opening, Mail{view, folder_navigation, ..}, Calendar{..}}` state.rs:58-71.
- workspace/render.rs: `render_navigation` (:17) = column w 16rem (min 14, max 20), bg `sidebar`, border right; children: brand text, feature nav (Mail, Calendar ghost Buttons with icons), account identity row, `MailFolderNavigation` entity (Mail only) else flex_1 filler, bordered Accounts button, bordered Settings button. `render_navigation_button` (:141) = `Button::new(id).ghost().selected(..).label().icon(..)`. `render_workspace_content` (:163) centers Accounts both axes in a scroll viewport (the "narrow column" look).
- `Workspace::new` hardcodes `content: Accounts`; preview always starts on Accounts.
- Theme: `apply_appearance` (workspace.rs:148) calls `Theme::change(mode)` then `Theme::update(|t| t.ring = t.primary)`; re-applied on system appearance change. Only customization today.
- `NavigationSurfaceAppearance {Opaque, Translucent}` state.rs:22-46 (sidebar.opacity(0.88)); preview forces Opaque.
- bootstrap.rs: `gpui_kit::init`, `open_workspace_window`, `window_options()`; preview path `run_preview_application()` (:182), env `EINFACH_MAIL_UI_PREVIEW_SCENARIO` (standard | mail-sync-recovery).

### Theme capability (gpui-component 0.9.1)
- Custom theme without fork: `ThemeRegistry::global_mut(cx).load_themes_from_str(json)` with a `ThemeSet` (schema src/theme/schema.rs:40-270: `ThemeConfig { mode, font_family, radius, radius_lg, colors{ "accent.background", "sidebar.*", "primary.*", ... } }`), or `Theme::update` / `Theme::apply_config`. `Theme::change(mode)` reloads, so overrides must be re-applied after each change (already the pattern).
- Views read `cx.theme().semantic_tokens().colors` (ColorTokens: background, surface, border, foreground, muted, muted_foreground, accent, accent_foreground, primary, destructive, ring, sidebar ...) and `theme.radius`. Spacing tokens are fixed in gpui-component.
- Available but unused components: `sidebar` (Sidebar, SidebarMenu, SidebarMenuItem .icon .active .suffix), `badge`, `tag`, `avatar`, `tab`, `list`, `group_box`, `separator`, `setting`, `switch`, `toolbar`, `status_bar`. Used today: Button, Icon/IconName, RadioGroup, h_resizable, scrollable, Form/Field/Input, v_virtual_list.

### Mail (apps/desktop/src/features/mail/)
- `MailFolderNavigation` (view/folder_navigation.rs:17) is a separate entity rendered in the SHELL nav pane; flat ghost `Button` per folder (`folder:{id}`), "Folders" heading, Role::Group "Mail folders". No icons/badges/nesting.
- `MailView` (view.rs): constants `MESSAGE_ROW_HEIGHT` 4.5rem (feeds virtual list sizes), list width 22rem (18-30), reader min 30rem, `MAIL_SHELL_NAVIGATION_WIDTH` 16rem (couples compact breakpoint to nav width).
- `render_header` (view/render.rs:~20): "Mail" title, Read-only tag, sync status, primary Compose, ghost Sync.
- `render_cached_message_list` (view/message_list.rs:~160-285): `v_virtual_list` fixed-height rows; container Role::ListBox "Messages", track_focus, key_context "MailMessageList", actions prev/next/first/last/open. Row = `render_message_row_content(sender, subject)`: sender bold ALWAYS, subject; NO time; selected = `accent` bg; hover = `muted`.
- `message_sender()` (view.rs:496) returns "Name <addr>" string.
- Reader (view/message_reader.rs): header p_6 subject + text-only action buttons row (Read/Unread, Flag, Archive, Move to..., Reply, Delete danger), capability-gated; sanitized body (body.rs). Compact mode swaps list/reader with "Back to messages".
- Data available: subject, from/sender Mailbox (display_name, address), received_at SystemTime, read_state, follow_up_flag_status. NOT available: preview snippet, unread counts, folder kind, attachments, importance, avatars.
- Date grouping: jiff already a dependency; calendar/week.rs:41-47 has system tz with UTC fallback; helpers in calendar/time.rs are private to calendar.
- Must preserve (ADR 0024): ListBox/ListBoxOption roles, aria labels, position_in_set/size_of_set over a FLAT message index, `message-option-{id}` ids, `message-row-{id}` debug selectors, focus ring. Date headers must not break the flat index (render headers inside the first row of each group, or keep a separate index mapping and set sizes on messages only).

### Accounts / Settings / Calendar
- Accounts: features/account/render.rs (905 lines), `render_header` uses `max_w_48`; centered by workspace wrapper. Cards `account_identity`, `render_permissions` (rounded_lg, p_4, surface, border).
- Settings: features/settings/view.rs, `max_w_64` column, "Settings" text_2xl, one Appearance card (`settings-appearance-group`, surface, border, radius, p_4, RadioGroup System/Light/Dark), ad-hoc notice `settings-appearance-notice` (border_l_2 destructive).
- Calendar: features/calendar/view/render.rs, own header style (title, Read-only, actions, New event, Sync), agenda rows `rounded_md`.
- Repeated ad-hoc status/notice divs with Role::Status/Alert across account, settings, calendar. `components/` has only list_navigation.rs and sync_age.rs.

### Verification harness
- Preview: `OPENSSL_NO_VENDOR=1 cargo run --locked -p einfach-mail-desktop --features ui-preview --bin einfach-mail` (system OpenSSL needed on this machine). Synthetic data; seed in app/preview.rs (`seed_mail_cache`, message `preview-message-1`).
- No start-destination env var; keyboard injection via Hyprland did not switch views. A ui-preview-only env var (e.g. `EINFACH_MAIL_UI_PREVIEW_DESTINATION=mail|calendar|accounts|settings`) read in bootstrap would let `grim` screenshot each view.
- Headless tests: gpui_kit::test, `simulate_resize`, `window.click("nav-settings")`, `window.find(id)`, `find_all_by_role`, `debug_bounds`; layout/contrast checks in app/workspace/tests/appearance.rs (keeps ids `settings-appearance-group`, `settings-appearance-notice`, `nav-*`). Restyle must keep or deliberately update these.
