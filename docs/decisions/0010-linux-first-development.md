# ADR 0010: Focus current development on Linux

- Status: Accepted
- Date: 2026-10-31
- Owners: Project maintainer

## Context

The project foundation includes a native GPUI Kit shell that has been launched on Fedora Linux. Windows and macOS launch, authentication, credential storage, installation, and platform integration need their own environments and evidence. Keeping those checks in the current milestone increases workflow and planning cost before the first Linux mail workflow exists.

The product remains a native Rust desktop client for Microsoft 365. Linux-first development does not change the GPUI Kit choice, Exchange Online provider, security requirements, or the plan to support other desktop platforms later.

## Decision

Use Linux as the sole current development and validation target. Complete the Linux workflows and their security proofs before starting Windows or macOS implementation work. Keep CI on Ubuntu while the active milestone is Linux-first.

Treat the Linux GPUI shell foundation as complete for M0. Move the Windows interactive launch check to M5. Plan macOS support after the core workflow, under [M6](https://github.com/1fach/einfach-mail/milestone/6). Revisit platform-specific libraries and integration requirements when each support milestone starts.

Before production persistence stores real mailbox data, prove the Linux keyring and SQLCipher key lifecycle using the Linux Secret Service backend. Keep the accepted Windows storage proof as evidence for that platform only. Likewise, authentication and Graph transport need to pass their Linux-specific proofs before implementation proceeds; Windows and macOS behavior remains unclaimed.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Linux-first development | One active desktop environment and CI platform; focused evidence for the first product workflows | Windows and macOS support arrive later and require separate launch, security, packaging, and integration proofs |
| Maintain all desktop platforms in the current milestone | Earlier cross-platform build coverage | Increases workflow and review surface before any mail or calendar workflow exists; interactive launch and OS service behavior still require native environments |

## Consequences

- Run CI on Ubuntu Linux for the active milestone. Continue local Linux formatting, Clippy, documentation, dependency, security, and workspace checks.
- Do not claim Windows or macOS support based on Linux CI or cross-compilation alone.
- Do not store real mailbox data until the Linux operating-system credential path and missing-key behavior are proved.
- Update the roadmap and move platform-specific work to M5 (Windows) and M6 (macOS and broader release targets).
- Restore platform checks and review the relevant OS libraries when the corresponding support milestone begins.

## Sources

- [Project brief](../product/brief.md)
- [Project roadmap](../project/roadmap.md)
- [Project status](../project/status.md)
- [Linux credential and cache key lifecycle proof](https://github.com/1fach/einfach-mail/issues/20)
- [Deferred Windows shell launch check](https://github.com/1fach/einfach-mail/issues/2)
- [GPUI Kit platform setup](https://longbridge.github.io/gpui-component/)
