# Fluent workspace restyle

Status: accepted design, implementation in progress under [#451](https://github.com/1fach/einfach-mail/issues/451).

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

## Progress log

Each unit appends one line when it merges: date, unit, PR, and verification evidence.

- 2026-11-13: The design was produced by three `how` explorers and a two-candidate architect arena. No cross-judge ran because only Claude models were configured. The baseline screenshot [`fluent-baseline-accounts-dark.png`](../project/evidence/fluent-baseline-accounts-dark.png) shows the Accounts view: centered nav text buttons and a narrow centered column.
- 2026-11-13: U1–U3 foundation, [PR #463](https://github.com/1fach/einfach-mail/pull/463) for [#452](https://github.com/1fach/einfach-mail/issues/452). ADR 0066 accepted. `scripts/preview-shots.sh` captured Mail, Mail with reader, Calendar, Accounts, and Settings in light and dark (`docs/project/evidence/fluent-foundation-*.png`). The theme JSON installs once at startup, and contrast, exact-value, and no-raw-color tests pass. No view changed structure.
- 2026-11-13: U4 navigation pane and destination switcher, [PR #465](https://github.com/1fach/einfach-mail/pull/465) for [#453](https://github.com/1fach/einfach-mail/issues/453). `NavItem` (filled selection plus accent pill, `unavailable(reason)`) replaces the nav and folder buttons. The pane is account header with Avatar, New mail (Mail only), a Folders list, and a bottom icon switcher. `NAVIGATION_PANE_WIDTH` drives the pane and the compact Mail breakpoint, and the window has a minimum size. Headless tests cover pill bounds, switcher names, tab order, and the compact threshold. Screenshots: `docs/project/evidence/fluent-nav-*.png`. The preview account is read-only, so `fluent-nav-new-mail-*.png` was captured with a temporary local full-access preview profile that is not committed.
- 2026-11-13: U5 Accounts and Settings pages, [PR #468](https://github.com/1fach/einfach-mail/pull/468) for [#454](https://github.com/1fach/einfach-mail/issues/454). `Page`, `SettingsCard`, and `InfoBar` replace the hand-built frames and notices, and Accounts starts at the top of a 40rem column. Desktop tests went from 143 to 148 (158 with `ui-preview`) with card and info bar layout checks in both themes at 1280px and 640px. `docs/project/evidence/fluent-pages-*.png` shows Accounts and Settings in light and dark at 1280x800 and 760x700, plus the disconnected provider cards and the Exchange form.
- 2026-11-13: Integration of [PR #465](https://github.com/1fach/einfach-mail/pull/465) and [PR #468](https://github.com/1fach/einfach-mail/pull/468) into one stack. Bottom switcher items show selection with the fill alone and folder items keep the pill. The selected navigation fill is now `#D4D4D4` in light and `#3C3C3C` in dark, a test requires a 7 unit CIE lightness gap from hover. `SettingsCard` descriptions use the small body size. Desktop tests: 159, and 168 with `ui-preview`. `docs/project/evidence/fluent-nav-*.png` and `fluent-pages-*.png` were recaptured from the integrated build.
- 2026-11-13: U6 message list date groups and rows, [PR #TBD](https://github.com/1fach/einfach-mail/pull/TBD) for [#455](https://github.com/1fach/einfach-mail/issues/455). `LocalClock` moves from `calendar/week.rs` to `components/local_time.rs`. `MessageDay` is computed once in `MailView::set_messages`, row sizes derive from it, and the day header renders inside virtual item i above its `list_option`, so the flat index, ADR 0024 keys, set positions and `message-*` ids are unchanged. `PaneHeader` replaces the Mail header and now holds Sync, Narrow list and Widen list. Rows show a small Avatar, the sender name, the time, the subject and a flag icon, and unread rows add an accent dot to the semibold text. Desktop tests went from 159 to 176 (185 with `ui-preview`), and `scripts/check-linux-local.sh` passes including `cargo audit`. `docs/project/evidence/fluent-list-*.png` shows Mail and Mail with reader in light and dark at 1280x800, plus the list at 760x700. The preview font did not render semibold, so unread rows read through the dot and the accent time in the shots.
