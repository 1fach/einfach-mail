# Product brief

## Purpose

Einfach Mail is a desktop workspace for email and calendar. It aims to pair a familiar, keyboard-friendly mail client with direct Microsoft 365 integration and useful operating-system behavior.

The product name is Einfach Mail. The first product decision is to make Exchange Online work well. The app will use Microsoft Graph and delegated access for the signed-in user.

## First users

Develop and validate the first workflows on Linux for people who use Microsoft 365 work or school accounts. Add Windows and macOS support in later milestones. Keep Windows-specific Outlook integration in the product plan. Personal Microsoft accounts and Exchange Server installed on premises need explicit acceptance tests before they become supported targets.

## User outcomes

- Connect a Microsoft account with the system browser and see what access the app requests.
- Read and organize mail without waiting for every screen to make a network request.
- Read and manage a calendar in the same application.
- Create and send mail and create or update events with clear success and failure states.
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

The initial release plan covers one signed-in Microsoft account, inbox and folder browsing, message reading, basic message actions, calendar viewing and event editing, and a local cache. Build and verify the first workflows on Linux. Add Windows desktop integration and macOS support in later milestones. The roadmap sequences these workflows. It does not promise all of them in the first runnable prototype.

## Out of scope until a decision changes the brief

- Exchange Server installed on premises.
- Full Outlook or Thunderbird feature parity.
- Mobile clients, team administration, and server-side multi-user access.
- Add-ins, arbitrary plug-ins, and extensions that execute inside email content.
- Application permissions that let the app access mailboxes without the signed-in user.

## Open questions

- Should the first account type include personal Outlook.com accounts as well as work or school accounts?
- Which Windows-specific features matter most: notifications, taskbar unread count, startup behavior, or calendar reminders?
- Should the first release support multiple accounts?
- Which offline actions may queue, and how should the UI explain conflicts?
- What license should the project use?
