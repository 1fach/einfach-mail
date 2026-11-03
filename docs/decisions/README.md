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
| [0012](0012-application-owned-mail-sessions.md) | Keep mail sessions behind application workflows | Accepted |
| [0013](0013-graph-account-identity.md) | Use the Graph profile for initial account identity | Accepted for Linux; live Entra verification open |
| [0024](0024-keyboard-navigation-for-cached-messages.md) | Provide keyboard navigation for cached messages | Accepted |
| [0030](0030-account-operation-cancellation-boundary.md) | Order account cancellation against credential commits | Accepted for the Linux M1 account flow |
| [0031](0031-registered-loopback-port-for-entra.md) | Require a registered loopback port for Entra | Accepted for Linux; tenant verification open |
| [0032](0032-recover-expired-message-delta-cursors.md) | Recover expired message delta cursors | Accepted for Linux; live Graph verification remains open |
| [0034](0034-invalidate-cached-message-bodies-after-delta-updates.md) | Invalidate cached message bodies after delta updates | Accepted for Linux implementation |
| [0037](0037-reject-stale-mail-sync-pages.md) | Reject stale mail sync pages | Accepted for Linux implementation |
