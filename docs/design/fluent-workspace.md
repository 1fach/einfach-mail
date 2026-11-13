# Fluent workspace restyle

Status: implemented under [#451](https://github.com/1fach/einfach-mail/issues/451). The [Review](#review) section records what was checked and the known gaps.

Einfach Mail should look and feel like a Windows 11 app on Linux, using Wino Mail and the Windows Mail client as visual references. Wino Mail (`repos/wino-mail/`) is GPL-3.0 and is a reference only. Do not copy its code or assets. This restyle uses only data the app already has. New mail data, such as preview text and unread counts, is tracked separately.

## Read in this order

1. [Synthesis](fluent-workspace/synthesis.md) is the accepted design. It names the base candidate, the grafts, the rejected parts, the defaults, and the implementation units U1–U9.
2. [Candidate A](fluent-workspace/candidate-a.md) is the base design. It holds the token table, primitive signatures, per-screen layout, accessibility contract, and verification lever. The synthesis overrides it where they differ.
3. [Grounding](fluent-workspace/grounding.md) maps the current code and lists the constraints both candidates worked from.
4. [Candidate B](fluent-workspace/candidate-b.md) is the alternative design. Read it for the grafted ideas and the rejected alternatives.

[ADR 0066](../decisions/0066-fluent-visual-language-and-navigation.md) records the lasting decision. It covers the Fluent visual language and amends ADR 0059's navigation selection and destination placement.

## Decisions from the repository owner

- Restyle with existing data first. Track the data additions as separate issues, each with its own ADR.
- Amend ADR 0059. Navigation uses a filled selection surface with a short accent pill and a bottom destination switcher for Mail, Calendar, Accounts, and Settings.

## Defaults chosen during design

The owner can change these. Record any change here and in ADR 0066.

- The base font stays 16px and uses the platform UI font.
- The navigation pane is a fixed 16rem with no icon rail.
- Message times use a 24-hour clock and English date labels until localization.
- Delete is a neutral ghost button because it moves the message to Deleted Items.
- Icons come from a curated Lucide subset, not the full `AllAssets` bundle.

## Issue map

| Unit | Issue | Depends on |
|---|---|---|
| U1–U3 Foundation: ADR 0066, preview screenshot lever, theme token layer | [#452](https://github.com/1fach/einfach-mail/issues/452) | none |
| U4 Navigation pane and destination switcher | [#453](https://github.com/1fach/einfach-mail/issues/453) | #452 |
| U5 Accounts and Settings pages | [#454](https://github.com/1fach/einfach-mail/issues/454) | #452 |
| U6 Message list date groups and rows | [#455](https://github.com/1fach/einfach-mail/issues/455) | #453 |
| U7 Reader command bar | [#456](https://github.com/1fach/einfach-mail/issues/456) | #453 |
| U8 Calendar alignment | [#457](https://github.com/1fach/einfach-mail/issues/457) | #455 |

Data follow-ups, outside this restyle: preview line [#458](https://github.com/1fach/einfach-mail/issues/458), folder unread counts [#459](https://github.com/1fach/einfach-mail/issues/459), well-known folder kinds [#460](https://github.com/1fach/einfach-mail/issues/460), attachments [#461](https://github.com/1fach/einfach-mail/issues/461), and importance [#462](https://github.com/1fach/einfach-mail/issues/462).

U4 and U5 can run in parallel, and so can U6 and U7. U4 and U5 both edit `app/workspace/render.rs` in different functions, so merge U4 first.

## Verification

Build the synthetic preview with `OPENSSL_NO_VENDOR=1` when Perl `FindBin` is missing:

```sh
OPENSSL_NO_VENDOR=1 cargo run --locked --package einfach-mail-desktop --features ui-preview --bin einfach-mail
```

On Hyprland, `hyprctl clients -j` gives the window geometry and `grim -g "<x>,<y> <w>x<h>" out.png` captures it. Keyboard events sent with `hyprctl dispatch` did not reach the GPUI window during design. The foundation unit therefore adds preview-only environment variables that choose the start view, appearance, and window size. Use synthetic preview data only.

## Review

Closeout of [#451](https://github.com/1fach/einfach-mail/issues/451) on 2026-11-14. Each item below was checked against the screenshots in `docs/project/evidence/fluent-*.png` (captured by `scripts/preview-shots.sh`; the full-access shots use `EINFACH_MAIL_UI_PREVIEW_SCENARIO=full-access`) and against the headless GPUI tests. No live screen-reader session was run, and no live provider was used.

### Accessibility checklist

| Item | Result | Evidence |
|---|---|---|
| Every action reachable and operable by keyboard | Pass for the restyled surfaces. Not exercised in a real window. | Headless tests cover the destination switcher, the pane tab order, the message list and the calendar grid as one tab stop each, the mini month (arrows, Enter), the reader command bar and its More menu, and the width controls. |
| Focus order follows visual and task order | Pass | `tab_order_follows_the_pane_from_top_to_bottom`, and the list, grid and mini month tests. |
| Focus visible and restored after overlays | Visible: pass. Restored: not re-reviewed here. | The ring colour has a contrast test in both themes. Sheet focus restore was not part of the restyle and has no new test. |
| Accessible names; icon-only controls have tooltips | Pass | Icon controls (Sync, list width, Today, previous and next week or month, More actions) carry names and tooltips. Day cells, day headers and event options have full-date names. The mini month weekday initials now read as full weekday names (`mini_month_weekday_headers_are_named_with_full_weekday_names`). |
| Contrast of text and meaningful boundaries | Pass | `fluent_palette_meets_contrast_minimums_in_light_and_dark`, the focus ring and status colour test, and the 7 unit CIE lightness gap between selection and hover. |
| Status not conveyed by colour alone | Pass | Unread rows add a dot, today is named ("today") and filled, notices carry an icon and text, and the selected navigation item has a fill plus a pill on folders. |
| Disabled and read-only states distinguishable | Pass | Read-only is a badge in the Mail and Calendar headers, controls that need a missing capability are absent, and unavailable navigation items are disabled with a reason. |
| Labels, errors and descriptions near their controls | Pass | `SettingsCard` descriptions sit under the title, and notices (`InfoBar`) sit directly above the content they describe. |
| Longer translations and larger text | Not verified | The base font is fixed at 16px and there is no localization yet. Larger text and zoom were not tried, so this stays open under [#296](https://github.com/1fach/einfach-mail/issues/296). |
| Comfortable pointer targets | Pass by inspection, not asserted | Navigation rows are about 36px tall, mini month cells and header icon buttons about 32px. No test measures them. |
| Announcements | Pass | `calendar-sync-status` is a `Status` and `calendar-write-notice` is an `Alert` (`calendar_caption_is_a_status_and_the_write_notice_is_an_alert`). The Mail sync failure alert and retry status have tests. |

### Design review checklist

1. **Task clear.** Pass. Each destination has a titled pane, and the primary action is New mail or New event in the navigation pane, drawn only when the account can do it.
2. **Actions keep their promise.** Pass, with one caveat. Delete is neutral because it moves a message to Deleted Items. In the `full-access` preview each write reports that it is unsupported, so it checks layout only.
3. **Hierarchy restrained.** Pass. One filled primary button per pane, accent only for today, unread and selection.
4. **Do less, better.** Pass. The restyle added no controls without an action. Wino features with no data behind them (preview line, unread counts, attachments, importance) are issues #458–#462.
5. **Exact structure.** Pass by inspection. The date groups, headers and rows share left spines at 1280 and 760. No pixel measurement beyond the layout tests.
6. **Follows the component system.** Pass, with the gap below. Standard `Button`, `Avatar` and `Radio` keep their geometry and keyboard behaviour. New primitives (`NavItem`, `PaneHeader`, `CommandBar`, `InfoBar`, `SettingsCard`) take colours from theme slots only, and a source scan test forbids raw colours.
7. **Every state and constraint.** Partly verified. Loading, failure, read-only and offline copy exist for Mail and Calendar and have tests. Compact (760x700) and overflow (1180x800) layouts were captured. Larger text, longer translations and reduced motion were not tried.
8. **Tested in a real window.** Partly. Every view was captured in a real Hyprland window in light and dark at 1280x800 and 760x700, but a person has not completed a task with a keyboard or screen reader.

### Known gaps

- The selected navigation item is not exposed as `selected` to assistive technology, because GPUI Kit `Button` has no such state. The selection shows as a fill and, on folders, a pill.
- Semibold does not render with the system UI font in the captures, so unread rows rely on the accent dot and time colour, as designed.
- No live screen-reader review. Tracked by [#9](https://github.com/1fach/einfach-mail/issues/9) and [#22](https://github.com/1fach/einfach-mail/issues/22).
- The week grid has no time axis. Events are stacked by start time in each day column, as in the agenda, so their height says nothing about their length.
- The preview captures show a pink or red compositor border at the window corners. It comes from the Hyprland window border, not the app.
- The disconnected-account and Exchange form captures (`fluent-pages-accounts-disconnected-*`, `fluent-pages-accounts-ews-form-*`, `fluent-pages-narrow-accounts-disconnected-*`) were not recaptured, because the preview script cannot reach those states.

### Visual follow-ups noticed in the captures

- The Mail empty reader text ("Select a message to read it") sits at the top of the pane and is not centred.
- The compact agenda repeats the time zone abbreviation ("09:00 CET to 09:30 CET") on every event.
- At 760px the 16rem navigation pane keeps a third of the window, which leaves the list or reader about 500px. Wino collapses to an icon rail. This is the accepted fixed-pane default.

## Progress log

Each unit appends one line when it merges: date, unit, PR, and verification evidence.

- 2026-11-13: The design was produced by three `how` explorers and a two-candidate architect arena. No cross-judge ran because only Claude models were configured. The baseline screenshot [`fluent-baseline-accounts-dark.png`](../project/evidence/fluent-baseline-accounts-dark.png) shows the Accounts view: centered nav text buttons and a narrow centered column.
- 2026-11-13: U1–U3 foundation, [PR #463](https://github.com/1fach/einfach-mail/pull/463) for [#452](https://github.com/1fach/einfach-mail/issues/452). ADR 0066 accepted. `scripts/preview-shots.sh` captured Mail, Mail with reader, Calendar, Accounts, and Settings in light and dark (`docs/project/evidence/fluent-foundation-*.png`). The theme JSON installs once at startup, and contrast, exact-value, and no-raw-color tests pass. No view changed structure.
- 2026-11-13: U4 navigation pane and destination switcher, [PR #465](https://github.com/1fach/einfach-mail/pull/465) for [#453](https://github.com/1fach/einfach-mail/issues/453). `NavItem` (filled selection plus accent pill, `unavailable(reason)`) replaces the nav and folder buttons. The pane is account header with Avatar, New mail (Mail only), a Folders list, and a bottom icon switcher. `NAVIGATION_PANE_WIDTH` drives the pane and the compact Mail breakpoint, and the window has a minimum size. Headless tests cover pill bounds, switcher names, tab order, and the compact threshold. Screenshots: `docs/project/evidence/fluent-nav-*.png`. The preview account is read-only, so `fluent-nav-new-mail-*.png` was captured with a temporary local full-access preview profile that is not committed.
- 2026-11-13: U5 Accounts and Settings pages, [PR #468](https://github.com/1fach/einfach-mail/pull/468) for [#454](https://github.com/1fach/einfach-mail/issues/454). `Page`, `SettingsCard`, and `InfoBar` replace the hand-built frames and notices, and Accounts starts at the top of a 40rem column. Desktop tests went from 143 to 148 (158 with `ui-preview`) with card and info bar layout checks in both themes at 1280px and 640px. `docs/project/evidence/fluent-pages-*.png` shows Accounts and Settings in light and dark at 1280x800 and 760x700, plus the disconnected provider cards and the Exchange form.
- 2026-11-13: Integration of [PR #465](https://github.com/1fach/einfach-mail/pull/465) and [PR #468](https://github.com/1fach/einfach-mail/pull/468) into one stack. Bottom switcher items show selection with the fill alone and folder items keep the pill. The selected navigation fill is now `#D4D4D4` in light and `#3C3C3C` in dark, a test requires a 7 unit CIE lightness gap from hover. `SettingsCard` descriptions use the small body size. Desktop tests: 159, and 168 with `ui-preview`. `docs/project/evidence/fluent-nav-*.png` and `fluent-pages-*.png` were recaptured from the integrated build.
- 2026-11-13: U6 message list date groups and rows, [PR #473](https://github.com/1fach/einfach-mail/pull/473) for [#455](https://github.com/1fach/einfach-mail/issues/455). `LocalClock` moves from `calendar/week.rs` to `components/local_time.rs`. `MessageDay` is computed once in `MailView::set_messages`, row sizes derive from it, and the day header renders inside virtual item i above its `list_option`, so the flat index, ADR 0024 keys, set positions and `message-*` ids are unchanged. `PaneHeader` replaces the Mail header and now holds Sync, Narrow list and Widen list. Rows show a small Avatar, the sender name, the time, the subject and a flag icon, and unread rows add an accent dot to the semibold text. Desktop tests went from 159 to 176 (185 with `ui-preview`), and `scripts/check-linux-local.sh` passes including `cargo audit`. `docs/project/evidence/fluent-list-*.png` shows Mail and Mail with reader in light and dark at 1280x800, plus the list at 760x700. The preview font did not render semibold, so unread rows read through the dot and the accent time in the shots.
- 2026-11-13: U7 reader command bar, [PR #472](https://github.com/1fach/einfach-mail/pull/472) for [#456](https://github.com/1fach/einfach-mail/issues/456). `CommandBar` and `Command` (`components/command_bar.rs`) replace the hand-built action row: ghost icon and label buttons, with Read/Unread and Flag moving into a "More actions" menu when the reader is narrower than 40rem. The subject and sender sit below the bar, notices are `InfoBar`s, Delete is a neutral ghost button, and the compact back button leads the bar. Desktop tests went from 159 to 169 (168 to 178 with `ui-preview`). `docs/project/evidence/fluent-reader-*.png` shows the reader in light and dark at 1280x800, overflow at 1180x800, and compact at 1000x700 and 780x700. The preview account is read-only, so the shots used a temporary local full-access preview profile that is not committed.
- 2026-11-13: U8 Calendar alignment, [PR #475](https://github.com/1fach/einfach-mail/pull/475) for [#457](https://github.com/1fach/einfach-mail/issues/457). New event and a keyboard-operable mini month move into the navigation pane (`CalendarNavigation`, like Mail's folders), and the header is a `PaneHeader` with Today, previous and next week, and Sync as icon controls. `week_agenda` builds the flat event order once with a `WeekGrid` of seven `DayColumn`s, and the week renders as seven columns with event bars above 44rem of content width and as the `GroupHeader` agenda below it. Both layouts fill the same single listbox, so the ADR 0042 keys, Tab stop and set positions are unchanged. Notices are `InfoBar`s. Desktop tests went from 176 to 195 before merging #472 (205 after, 214 with `ui-preview`), and `scripts/check-linux-local.sh` passes. `docs/project/evidence/fluent-calendar-*.png` shows the grid in light and dark at 1280x800, New event with a temporary local full-access profile, and the agenda at 760x700.
- 2026-11-14: U9 closeout for [#451](https://github.com/1fach/einfach-mail/issues/451). `EINFACH_MAIL_UI_PREVIEW_SCENARIO=full-access` gives the synthetic account every mail action and calendar write, so New mail, the reader command bar and New event are reproducible, and `scripts/preview-shots.sh -e` passes a scenario and parks the pointer outside the window. Tests now pin the `Status` caption and `Alert` write notice in Calendar and the full weekday names of the mini month initials, which became column headers in the mini month grid. All `fluent-{list,reader,nav,pages,calendar}-*.png` evidence was recaptured from the script on `main`, and the captures from the old local profile were replaced by full-access captures. The [Review](#review) section records the checklists and the known gaps. Desktop tests went from 205 to 207 (214 to 218 with `ui-preview`), and `scripts/check-linux-local.sh` passes including `cargo audit`.
