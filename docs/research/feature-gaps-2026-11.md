# Mail and calendar feature gaps, November 2026

Date: 2026-11-14. This research compares Einfach Mail with Thunderbird's EWS client and Wino Mail's Microsoft Graph sync. Wino has no EWS client. Thunderbird has no EWS calendar. The reference trees are `repos/thunderbird/` and `repos/wino-mail/`, used for reading only. Microsoft documentation and the product brief stay authoritative. The tracking epic is [#490](https://github.com/1fach/einfach-mail/issues/490).

## Owner decisions

- Lift the product brief's exclusion of EWS mail writes. An ADR amends the brief and ADRs 0053 and 0058. EWS writes follow Thunderbird's EWS behavior and keep the same guards as the existing EWS read-state write: version gates, receipt safety, no replay of uncertain writes, and recovery markers. [#491](https://github.com/1fach/einfach-mail/issues/491) blocks every EWS write issue.
- File contacts (recipient suggestions) as a design issue that needs an ADR. Tasks stay out of scope.

## Matrix

Legend: Y = yes, P = partial, N = no. Sources are Thunderbird (TB, `repos/thunderbird/rust/...`) and Wino (`repos/wino-mail/src/Wino.Core/Synchronizers/OutlookSynchronizer.cs`, cited as W:line). Graph parts use v1.0 only. Line numbers come from the pinned snapshots.

| Capability | Einfach Graph | Einfach EWS | TB EWS | Wino Graph | TB Graph | Existing issue |
|---|---|---|---|---|---|---|
| Folder sync | Y (full list every sync) | Y | Y delta (`ews_xpcom/src/client/sync_folder_hierarchy.rs`) | Y delta (W:2984, W:3201) | Y delta | none |
| Folder create, rename, delete | N | N | Y (`create_folder.rs`, `update_folder.rs`, `erase_folder/delete_folder.rs`) | Y (W:3932-3967) | Y | none |
| Folder move or copy | N | N | Y (`copy_move_operations/copy_move_folder.rs`) | N | Y | none |
| Empty folder | N | N | Y (`erase_folder/empty_folder.rs`) | Y (W:3944) | Y | none |
| Message delta sync | Y | Y | Y | Y | Y | — |
| Read/unread | Y | P (version gated) | Y | Y | Y | — |
| Mark all read | N | N | Y (`change_read_status_all.rs`, MarkAllItemsAsRead) | Y (W:3947) | Y | none |
| Flag | Y | N | Y (`change_flag_status.rs`) | Y | Y | #32 |
| Junk / not junk | N | N | Y (`mark_as_junk.rs`: MarkAsJunk on 2013+, else a move) | Y (beta `reportMessage`, W:3427) | P (a move) | none |
| Categories | N | N | N | Y (W:3040, W:3662-3724 master categories) | N | none |
| Focused/Other | N | n/a | n/a | Y (W:3506-3552) | N | none |
| Move | Y | N | Y (`copy_move_item.rs`) | Y | Y | #34 |
| Copy message | N | N | Y | N | Y | none |
| Delete (to trash) | Y | N | Y (`delete_messages.rs`; soft = a move to trash) | Y | Y | #36 |
| Permanent delete | N | N | Y | P | Y | none |
| Archive | Y | N | P (a move) | Y (W:3655) | N | #38 |
| Send new | Y (plain text) | N | Y (`send_message.rs`, MIME SendOnly) | Y (W:3597) | Y | #28 |
| Reply | Y (plain text) | N | P (MIME) | Y | ? | #30 |
| Reply all, forward | N | N | P (MIME) | Y (W:3560-3595) | ? | none |
| Drafts on the server | N | N | Y (`create_message.rs` SaveOnly) | Y (W:3560, `OutlookDraftUpdates.cs`) | P | none |
| Attachments view, download | N | N | P (MIME) | P (FileAttachment only, W:3876-3912) | ? | #461 (marker only) |
| Attachments send, large upload | N | N | P (MIME) | Y (upload sessions, W:3620-3650) | ? | none |
| HTML compose | N (plain text, ADR 0017) | N | Y (MIME) | Y (W:3776) | n/a | none |
| Signatures | N | N | ? | ? | ? | none |
| Aliases / send-as | N | N | N | Y (W:3342-3360) | N | none |
| Server search | N | N | N (local only) | Y (`$search`, W:4457) | N | none |
| Notifications (new mail) | N | N | n/a | ? | n/a | none (brief wants system notifications) |
| Push subscriptions | N | N | N | N | N | — (neither reference client has them) |
| Throttling backoff | ? | ? | Y (ErrorServerBusy BackOffMilliseconds, `operation_sender.rs:86,443`) | Y (MSAL and Graph) | ? | none |
| EWS invalid sync state recovery | n/a | ? | N (enum only) | n/a | n/a | none |
| EWS auth | n/a | NTLM only | Basic, NTLM, OAuth2 Bearer (`protocol_shared/src/authentication`) | n/a | n/a | #257 |
| Calendar multiple and colors | N | N | n/a | Y (W:5037-5141) | P | #467 |
| Calendar views (month, day) | N | N | n/a | ? | ? | #466 |
| Event create, update, delete | Y (timed standalone) | N (disabled, M11) | n/a | Y | N | #26, #40, #42, #406-#408 |
| Recurring event writes | N (display only) | N | n/a | Y (W:5283) | N | #6 (display) |
| Invites (respond), attendees | N | N (excluded) | n/a | Y (W:5297-5349, W:5256) | P | none |
| Reminders | N | N | n/a | P (one reminder, W:5268) | P | none |
| Online meetings | N | N | n/a | P (W:5221) | P | none |
| All-day event writes | N | N | n/a | Y | N | none |
| Contacts (recipient suggestions) | N | N | N | Y (W:1249-1281) | N | none |

## Issues

No open issue already covered any planned item, so all 27 are new. Each is a sub-issue of the epic.

| Item | Area | Issue | Title |
|---|---|---|---|
| 1 | Decision | [#491](https://github.com/1fach/einfach-mail/issues/491) | [Decision] Allow EWS mail writes following Thunderbird's EWS client |
| 2 | EWS | [#492](https://github.com/1fach/einfach-mail/issues/492) | [EWS] Enable flag, move, delete to Deleted Items and archive on EWS |
| 3 | EWS | [#493](https://github.com/1fach/einfach-mail/issues/493) | [EWS] Send and reply over EWS |
| 4 | Mail | [#494](https://github.com/1fach/einfach-mail/issues/494) | [Mail] Reply all and forward |
| 5 | Mail | [#495](https://github.com/1fach/einfach-mail/issues/495) | [Mail] Save drafts on the server |
| 6 | Mail | [#496](https://github.com/1fach/einfach-mail/issues/496) | [Mail] Attachments: list, download, open safely and send |
| 7 | Mail | [#497](https://github.com/1fach/einfach-mail/issues/497) | [Mail] Mark all messages in a folder as read |
| 8 | Mail | [#498](https://github.com/1fach/einfach-mail/issues/498) | [Mail] Junk and not junk |
| 9 | Mail | [#499](https://github.com/1fach/einfach-mail/issues/499) | [Mail] Copy a message to another folder |
| 10 | Mail | [#500](https://github.com/1fach/einfach-mail/issues/500) | [Mail] Folder management |
| 11 | Mail | [#501](https://github.com/1fach/einfach-mail/issues/501) | [Mail] Permanent delete and empty Deleted Items |
| 12 | Mail | [#502](https://github.com/1fach/einfach-mail/issues/502) | [Mail] Server-side message search |
| 13 | Mail | [#503](https://github.com/1fach/einfach-mail/issues/503) | [Mail] Categories |
| 14 | Mail | [#504](https://github.com/1fach/einfach-mail/issues/504) | [Mail] Focused and Other inbox |
| 15 | Mail | [#505](https://github.com/1fach/einfach-mail/issues/505) | [Mail] Send from aliases |
| 16 | Mail | [#506](https://github.com/1fach/einfach-mail/issues/506) | [Mail] Rich text (HTML) compose |
| 17 | Mail | [#507](https://github.com/1fach/einfach-mail/issues/507) | [Mail] Signatures |
| 18 | Mail | [#508](https://github.com/1fach/einfach-mail/issues/508) | [Mail] Desktop notifications for new mail |
| 19 | EWS | [#509](https://github.com/1fach/einfach-mail/issues/509) | [EWS] Honor ErrorServerBusy BackOffMilliseconds throttling |
| 20 | EWS | [#510](https://github.com/1fach/einfach-mail/issues/510) | [EWS] Recover from ErrorInvalidSyncStateData with a full resync |
| 21 | EWS | [#511](https://github.com/1fach/einfach-mail/issues/511) | [EWS] Authentication beyond NTLM: Basic over TLS and OAuth2 |
| 22 | Mail | [#512](https://github.com/1fach/einfach-mail/issues/512) | [Mail] Delta sync for the folder hierarchy on Graph |
| 23 | Calendar | [#513](https://github.com/1fach/einfach-mail/issues/513) | [Calendar] Create and edit recurring events on Graph |
| 24 | Calendar | [#514](https://github.com/1fach/einfach-mail/issues/514) | [Calendar] Meeting invites on Graph: attendees and responses |
| 25 | Calendar | [#515](https://github.com/1fach/einfach-mail/issues/515) | [Calendar] Event reminders and online meeting toggle |
| 26 | Calendar | [#516](https://github.com/1fach/einfach-mail/issues/516) | [Calendar] Create and edit all-day events on Graph |
| 27 | Contacts | [#517](https://github.com/1fach/einfach-mail/issues/517) | [Contacts] Contacts sync for recipient suggestions |
