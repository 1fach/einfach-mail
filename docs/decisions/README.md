# Architecture decision records

An ADR records one decision, its context, alternatives, consequences, and status. Number files in order. Use `Proposed`, `Accepted`, `Superseded`, or `Rejected` as the status.

Keep a decision narrow enough that a later change can supersede it without rewriting unrelated history. Product goals belong in `docs/product/brief.md`; current work belongs in `docs/project/status.md`.

Copy [the ADR template](TEMPLATE.md) for each decision. Name new files `NNNN-short-title.md`. Update the index in the same change. Keep research and candidates in `docs/project/dependency-review.md` until the project accepts a choice.

## Index

| ID | Decision | Status |
| --- | --- | --- |
| [0001](0001-gpui-desktop-shell.md) | Use GPUI Kit for the native desktop shell | Accepted |
| [0002](0002-exchange-online-provider.md) | Use Microsoft Graph v1.0 for Exchange Online | Accepted |
| [0003](0003-desktop-authentication.md) | Desktop authentication implementation | Accepted for Linux; live Entra validation remains open |
| [0004](0004-local-data-and-sync.md) | Keep a local cache and synchronize with Graph | Accepted; encryption detailed in ADR 0005 |
| [0005](0005-encrypted-local-cache.md) | Use SQLCipher-backed SQLite for the local cache | Accepted for Linux based on the Linux Secret Service proof |
| [0006](0006-graph-http-transport.md) | Use reqwest for Microsoft Graph transport | Accepted for Linux; live Graph behavior remains unverified |
| [0007](0007-message-body-rendering.md) | Render sanitized message HTML in native GPUI | Accepted and implemented for Linux; manual graphical evidence pending |
| [0008](0008-identity-token-boundary.md) | Keep identity tokens inside the Microsoft provider | Accepted |
| [0009](0009-account-scoped-domain-identifiers.md) | Use account-scoped opaque domain identifiers | Accepted |
| [0010](0010-linux-first-development.md) | Focus current development on Linux | Accepted |
| [0011](0011-initial-mail-sync-scope.md) | Define the initial read-only mail synchronization scope | Accepted |
| [0012](0012-application-owned-mail-sessions.md) | Keep mail sessions behind application workflows | Superseded by ADR 0043 |
| [0013](0013-graph-account-identity.md) | Use the Graph profile for initial account identity | Accepted for Linux; live Entra verification open |
| [0014](0014-linux-read-only-calendar-workflow.md) | Implement a cached Linux read-only primary calendar | Accepted for Linux; live Graph and manual UI verification remain open |
| [0015](0015-idempotent-message-read-state-update.md) | Add idempotent message read-state updates | Accepted |
| [0016](0016-idempotent-primary-calendar-event-creation.md) | Create one-off timed events in the primary calendar | Accepted; tenant verification remains open |
| [0017](0017-safe-plain-text-mail-sending.md) | Send one plain-text message through Microsoft Graph | Accepted; live verification remains open |
| [0018](0018-native-plain-text-message-reply.md) | Reply to one cached message through Microsoft Graph | Accepted; tenant verification remains open |
| [0019](0019-follow-up-flags-on-cached-messages.md) | Set follow-up flags on cached messages | Accepted; live verification remains open |
| [0020](0020-move-cached-messages-between-folders.md) | Move cached messages between visible folders | Accepted; tenant verification remains open |
| [0023](0023-edit-standalone-calendar-appointments.md) | Edit standalone calendar appointments | Accepted; tenant verification remains open |
| [0024](0024-keyboard-navigation-for-cached-messages.md) | Provide keyboard navigation for cached messages | Accepted |
| [0026](0026-delete-standalone-calendar-appointments.md) | Delete standalone calendar appointments | Accepted; tenant verification remains open |
| [0027](0027-serialize-mail-account-removal-with-mail-operations.md) | Serialize mail account removal with mail operations | Accepted |
| [0028](0028-single-linux-desktop-process.md) | Allow one Linux desktop process per user | Accepted |
| [0029](0029-retry-cache-removal-after-sidecar-failure.md) | Keep cache removal retryable after a sidecar failure | Accepted |
| [0030](0030-account-operation-cancellation-boundary.md) | Order account cancellation against credential commits | Accepted for the Linux M1 account flow |
| [0031](0031-registered-loopback-port-for-entra.md) | Require a registered loopback port for Entra | Accepted for Linux; tenant verification open |
| [0032](0032-recover-expired-message-delta-cursors.md) | Recover expired message delta cursors | Accepted for Linux; live Graph verification remains open |
| [0034](0034-invalidate-cached-message-bodies-after-delta-updates.md) | Invalidate cached message bodies after delta updates | Accepted for Linux implementation |
| [0036](0036-recover-expired-calendar-delta-cursors.md) | Recover expired calendar delta cursors | Accepted for Linux read-only calendar synchronization; live Graph verification remains open |
| [0037](0037-reject-stale-mail-sync-pages.md) | Reject stale mail sync pages | Accepted for Linux implementation |
| [0038](0038-share-mail-cache-handles-by-account.md) | Share mail cache handles by account | Superseded by ADR 0043 |
| [0039](0039-move-messages-to-deleted-items.md) | Move messages to Deleted Items | Accepted; shared API superseded by ADR 0040 |
| [0040](0040-move-messages-to-the-default-archive-folder.md) | Move messages to the default Archive folder | Accepted |
| [0041](0041-bind-saved-sign-in-to-cache-identity.md) | Bind saved sign-in to a verified cache identity | Accepted for Linux M1 |
| [0042](0042-keyboard-navigation-for-calendar-agenda.md) | Use one listbox for weekly agenda navigation | Accepted; manual Linux accessibility review remains open |
| [0043](0043-per-account-mail-and-calendar-session.md) | Use one per-account mail and calendar session | Accepted |
| [0044](0044-block-calendar-actions-during-replacement-sync.md) | Block calendar actions during replacement sync | Accepted |
| [0045](0045-incremental-graph-permissions.md) | Request Graph permissions incrementally | Accepted; live consent verification remains open |
| [0046](0046-account-cache-removal-port.md) | Use one account cache removal port | Accepted |
| [0047](0047-share-write-recovery-across-account-sessions.md) | Share write readiness across account sessions | Accepted |
| [0048](0048-redact-sensitive-debug-output.md) | Redact sensitive domain Debug output | Accepted |
| [0049](0049-account-cache-open-errors.md) | Use account-scoped errors when opening the shared cache | Accepted |
| [0050](0050-name-gpui-date-pickers-through-component-patch.md) | Name styled GPUI DatePickers through a local API patch | Superseded by ADR 0055 |
| [0051](0051-defer-calendar-id.md) | Defer a calendar identifier until calendar selection exists | Accepted |
| [0052](0052-persist-calendar-event-write-recovery.md) | Persist calendar event write recovery | Accepted |
| [0053](0053-linux-read-only-exchange-ews.md) | Add Linux read-only Exchange Server mail through EWS | Accepted |
| [0054](0054-linux-exchange-ews-autodiscover.md) | Discover Linux EWS endpoints with Autodiscover | Accepted |
| [0055](0055-defer-date-picker-accessible-names.md) | Defer DatePicker accessible names without local vendoring | Accepted |
