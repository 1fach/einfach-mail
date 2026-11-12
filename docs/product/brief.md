# Product brief

## Purpose

Einfach Mail is a desktop workspace for email and calendar. It aims to pair a familiar, keyboard-friendly mail client with direct Microsoft 365 integration and useful operating-system behavior.

The product name is Einfach Mail. Exchange Online uses Microsoft Graph and delegated access for the signed-in user. Milestone M7 adds a separate Linux account path for mail from Exchange Server installed on premises through EWS. M8 adds read-only EWS calendar views in the shared native agenda; see [ADR 0057](../decisions/0057-read-only-ews-calendar-snapshots.md) and [issue #266](https://github.com/1fach/einfach-mail/issues/266). M9 adds EWS read-state changes only when the authenticated server reports a product build that supports read-receipt suppression; other EWS writes stay unavailable. See [ADR 0058](../decisions/0058-per-operation-ews-mail-capabilities.md) and [issue #267](https://github.com/1fach/einfach-mail/issues/267).

EWS calendar writes remain deferred while issue [#389](https://github.com/1fach/einfach-mail/issues/389) defines their operation and recovery boundaries.

## First users

Develop and validate the first workflows on Linux for people who use Microsoft 365 work or school accounts or Exchange Server installed on premises. Add Windows and macOS support in later milestones. Keep Windows-specific Outlook integration in the product plan. Personal Microsoft accounts remain out of scope. EWS live-server verification remains open until a test server or sanitized recording is available.

## User outcomes

- Connect a Microsoft account with the system browser and see what access the app requests.
- Read and organize mail without waiting for every screen to make a network request.
- Read and manage a calendar in the same application.
- Create and send mail and create, update, or delete supported calendar appointments with clear success and failure states.
- Keep recently synchronized data available when the connection is unavailable.
- Use keyboard navigation, system notifications, and native window behavior where they improve daily work.

## Product rules

- Start with a small, polished mail and calendar loop. Do not copy every Thunderbird or Outlook feature into the first release.
- Treat the Microsoft service as the authority for remote mail and events. Show pending local actions as pending until the service confirms them.
- Explain permission requests in the product and request only the delegated scopes needed for enabled features.
- Keep account secrets and tokens out of UI views and the local mail database.
- Treat message and event content as untrusted. Do not load remote content in email by default.
- Build the application interface in native GPUI views. Consider a browser engine only for rendering sanitized message HTML, with a separate security decision.
- Make sync status, offline state, and failed actions visible. Never imply that a queued action has reached the server.

## First-release scope

The first release supports one active signed-in account at a time. A user may connect a different identity after signing out through the existing account and cache removal flow; simultaneous accounts and in-app account switching are deferred. The Accounts destination shows the active identity without a selector. See [ADR 0060](../decisions/0060-one-active-account-first-release.md) and [issue #268](https://github.com/1fach/einfach-mail/issues/268).

The initial release plan also covers inbox and folder browsing, message reading, basic message actions, calendar viewing, and supported appointment editing and deletion. Calendar writes are limited to explicit timed, standalone appointments in the primary calendar with recoverable confirmation and sync states. On-premises Exchange accounts use EWS for mail and primary-calendar views. M9 adds read and unread state changes only when the server can suppress read receipts. EWS accounts do not expose other mail writes or calendar changes. Account setup can discover an EWS endpoint from the mailbox address or accept a manually entered HTTPS endpoint; external Autodiscover destinations require confirmation. Build and verify the first workflows on Linux. Add Windows desktop integration and macOS support in later milestones. The roadmap sequences these workflows. It does not promise all of them in the first runnable prototype.

## Out of scope until a decision changes the brief

- EWS mail writes other than the read-state action planned under M9.
- EWS calendar changes until issue #389 accepts safe request and recovery rules.
- Full Outlook or Thunderbird feature parity.
- Multiple simultaneously connected accounts and in-app account switching; see [ADR 0060](../decisions/0060-one-active-account-first-release.md).
- Mobile clients, team administration, and server-side multi-user access.
- Add-ins, arbitrary plug-ins, and extensions that execute inside email content.
- Application permissions that let the app access mailboxes without the signed-in user.

## Open questions

- Should the first account type include personal Outlook.com accounts as well as work or school accounts?
- Which Windows-specific features matter most: notifications, taskbar unread count, startup behavior, or calendar reminders?
- Which offline actions may queue, and how should the UI explain conflicts?
- What license should the project use?
