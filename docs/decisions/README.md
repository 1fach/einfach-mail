# Architecture decision records

An ADR records one decision, its context, alternatives, consequences, and status. Number files in order. Use `Proposed`, `Accepted`, `Superseded`, or `Rejected` as the status.

Keep a decision narrow enough that a later change can supersede it without rewriting unrelated history. Product goals belong in `docs/product/brief.md`; current work belongs in `docs/project/status.md`.

Copy [the ADR template](TEMPLATE.md) for each decision. Name new files `NNNN-short-title.md`. Update the index in the same change. Keep research and candidates in `docs/project/dependency-review.md` until the project accepts a choice.

## Index

| ID | Decision | Status |
| --- | --- | --- |
| [0001](0001-gpui-desktop-shell.md) | Use GPUI Kit for the native desktop shell | Accepted |
| [0002](0002-exchange-online-provider.md) | Use Microsoft Graph v1.0 for Exchange Online | Accepted |
| [0003](0003-desktop-authentication.md) | Desktop authentication implementation | Proposed; spike required |
| [0004](0004-local-data-and-sync.md) | Keep a local cache and synchronize with Graph | Accepted; encryption design open |
