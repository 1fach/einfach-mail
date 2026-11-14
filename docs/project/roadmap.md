# Roadmap

Milestones describe product outcomes. Track active implementation tasks and their acceptance criteria in GitHub issues. Use [project status](status.md) for the current milestone and immediate next actions. Update this roadmap when milestone scope or sequencing changes.

## M0: Repository foundation

Provide a reproducible Rust workspace, native GPUI Kit shell, contributor workflow, and Linux development baseline. Linux is the current development target. See [ADR 0010](../decisions/0010-linux-first-development.md).

## M1: Identity and local data

Connect a supported Microsoft account through delegated access. Keep credentials and cached data within their approved security boundaries. See [ADR 0003](../decisions/0003-desktop-authentication.md), [ADR 0005](../decisions/0005-encrypted-local-cache.md), and [ADR 0008](../decisions/0008-identity-token-boundary.md).

## M2: Read-only mail

Let users browse folders, read synchronized messages from the offline cache, and view message content safely. Keep server-side mail changes out of this milestone. See [ADR 0006](../decisions/0006-graph-http-transport.md) and [ADR 0007](../decisions/0007-message-body-rendering.md).

## M3: Read-only calendar

Show and synchronize primary-calendar events in a weekly view. Preserve time-zone and recurrence behavior in the offline cache, and recover expired delta cursors. Keep event writes out of this milestone. See [ADR 0014](../decisions/0014-linux-read-only-calendar-workflow.md), [ADR 0036](../decisions/0036-recover-expired-calendar-delta-cursors.md), and [ADR 0042](../decisions/0042-keyboard-navigation-for-calendar-agenda.md).

## M4: Mail and calendar actions

Support explicit mail read-state and follow-up changes, sends, replies, moves, deletion, and archiving. Support creation, editing, and deletion of checked standalone timed calendar appointments. Keep uncertain writes non-retried and recover them through synchronization. See [ADR 0015](../decisions/0015-idempotent-message-read-state-update.md), [ADR 0016](../decisions/0016-idempotent-primary-calendar-event-creation.md), [ADR 0017](../decisions/0017-safe-plain-text-mail-sending.md), [ADR 0018](../decisions/0018-native-plain-text-message-reply.md), [ADR 0019](../decisions/0019-follow-up-flags-on-cached-messages.md), [ADR 0020](../decisions/0020-move-cached-messages-between-folders.md), [ADR 0023](../decisions/0023-edit-standalone-calendar-appointments.md), [ADR 0026](../decisions/0026-delete-standalone-calendar-appointments.md), [ADR 0039](../decisions/0039-move-messages-to-deleted-items.md), [ADR 0040](../decisions/0040-move-messages-to-the-default-archive-folder.md), and [ADR 0052](../decisions/0052-persist-calendar-event-write-recovery.md).

## M7: Exchange Server EWS support

Add a Linux account path for on-premises Exchange through EWS. Support NTLMv2 with TLS channel binding, read-only folder browsing, message synchronization, and message reading. Account setup can discover endpoints through unauthenticated HTTPS Autodiscover with confirmation before sending the mailbox address to a new origin, or use a manually entered HTTPS endpoint. Keep Exchange Online on Microsoft Graph. Defer read-only EWS calendar access to M8, mail writes, ADFS OAuth, Windows, and macOS. Keep the milestone open until a real Exchange server or sanitized recording verifies EWS and Autodiscover behavior. See [ADR 0053](../decisions/0053-linux-read-only-exchange-ews.md), [ADR 0054](../decisions/0054-linux-exchange-ews-autodiscover.md), and [issues #257](https://github.com/1fach/einfach-mail/issues/257) and [#258](https://github.com/1fach/einfach-mail/issues/258).

## M8: Read-only EWS calendar views

Add a Linux read-only primary-calendar agenda for on-premises Exchange accounts. Reuse the shared native agenda, display the read-only capability, and keep EWS event changes unavailable. Use EWS `FindItem` with `CalendarView` and atomically replace each fixed-range cache snapshot after confirming the full response is present. Preserve the current Graph delta flow. Do not claim live Exchange compatibility until a real-server response or sanitized recording verifies the request, recurrence expansion, and time-zone behavior. See [ADR 0057](../decisions/0057-read-only-ews-calendar-snapshots.md) and [issue #266](https://github.com/1fach/einfach-mail/issues/266).

## M9: EWS message read-state changes

Allow an EWS account to mark a message read or unread only when the authenticated server reports a product version that supports receipt suppression. The Linux implementation models mail actions individually, keeps sends, replies, flags, moves, deletes, and calendar writes unavailable for EWS, and does not retry uncertain outcomes. Synthetic SOAP and HTTPS transport tests verify request construction, response mapping, and recovery signaling. Live compatibility remains unverified under issue #257. See [ADR 0058](../decisions/0058-per-operation-ews-mail-capabilities.md) and [issue #267](https://github.com/1fach/einfach-mail/issues/267).

## M10: Native mail and calendar workspace

Bring account setup, Mail, the weekly Calendar agenda, and working preferences into one adaptive native GPUI workspace. Keep the mail list and reading pane together at desktop widths, preserve keyboard navigation and session ownership, and provide a useful narrow-window layout. The first release supports one active account at a time, with the active identity shown without a switcher. Use Linux system decorations and limit optional translucency to compact or transient surfaces with an opaque fallback. See [ADR 0059](../decisions/0059-native-mail-and-calendar-workspace.md), [ADR 0060](../decisions/0060-one-active-account-first-release.md), and the M10 work in [milestone 9](https://github.com/1fach/einfach-mail/milestone/9), including issues #268–#274 and #278–#280. The Fluent restyle of the shell, Accounts and Settings, message list, reader command bar and Calendar ([#451](https://github.com/1fach/einfach-mail/issues/451), [ADR 0066](../decisions/0066-fluent-visual-language-and-navigation.md)) is done for what the merged code shows and the screenshots confirm: a Windows 11 style navigation pane and cards, date-grouped message rows, a Fluent reader command bar and a seven-column week grid. It is not a Wino Mail clone. Message preview lines, unread counts and folder kinds (#458–#460, [ADR 0067](../decisions/0067-message-previews-folder-unread-counts-and-kinds.md)) are in the desktop, checked against synthetic data only. Attachments and importance wait on issues #461 and #462. Month, day and work-week views and multiple calendars wait on #466 and #467. The week grid has no time axis, and a real-window screen-reader review stays open under #9 and #22.

## M11: Safe EWS calendar actions (in progress)

Add create, update, and delete actions for one-off timed standalone appointments in the primary calendar for on-premises Exchange accounts. Gate each action independently. Never resend an uncertain EWS create; reconcile it by a positive marker lookup and keep an inconclusive operation unresolved. Use `NeverOverwrite` and `SendToNone` for updates, and move confirmed deletions to Deleted Items. Reject meetings, attendees, recurring and all-day events, and other unsupported shapes. Add synthetic SOAP fixtures before enabling any action, then verify live behavior against supported on-premises Exchange versions. EWS stays view-only until implementation review and live verification pass. Issue [#402](https://github.com/1fach/einfach-mail/issues/402) implements creation and marker-based recovery without enabling the capability. Issue [#403](https://github.com/1fach/einfach-mail/issues/403) implements guarded updates without enabling the capability; issue [#404](https://github.com/1fach/einfach-mail/issues/404) implements deletion. Issue [#405](https://github.com/1fach/einfach-mail/issues/405) adds the safe status-check UI. Track the work in [GitHub milestone 10](https://github.com/1fach/einfach-mail/milestone/10). [Issue #389](https://github.com/1fach/einfach-mail/issues/389) owns the policy; issues [#400–#405](https://github.com/1fach/einfach-mail/issues/400) track implementation, and issues [#406–#408](https://github.com/1fach/einfach-mail/issues/406) track separate live verification.

## M5: Windows support and daily-use integration

Add and verify Windows shell launch, authentication and credential storage, accessibility, installation, account removal, and operating-system integrations for mail and calendar use.

## M6: macOS and broader release targets

Add and verify macOS support. Evaluate other account types and release policies after the core workflow is reliable.
