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
