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

Add a Linux account path for on-premises Exchange through EWS. Support NTLMv2 with TLS channel binding, read-only folder browsing, message synchronization, and message reading. Account setup can discover endpoints through unauthenticated HTTPS Autodiscover with confirmation before sending the mailbox address to a new origin, or use a manually entered HTTPS endpoint. Keep Exchange Online on Microsoft Graph. Defer calendar access, mail writes, ADFS OAuth, Windows, and macOS. Keep the milestone open until a real Exchange server or sanitized recording verifies EWS and Autodiscover behavior. See [ADR 0053](../decisions/0053-linux-read-only-exchange-ews.md), [ADR 0054](../decisions/0054-linux-exchange-ews-autodiscover.md), and [issues #257](https://github.com/1fach/einfach-mail/issues/257) and [#258](https://github.com/1fach/einfach-mail/issues/258).

## M5: Windows support and daily-use integration

Add and verify Windows shell launch, authentication and credential storage, accessibility, installation, account removal, and operating-system integrations for mail and calendar use.

## M6: macOS and broader release targets

Add and verify macOS support. Evaluate other account types and release policies after the core workflow is reliable.
