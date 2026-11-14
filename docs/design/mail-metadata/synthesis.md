# Mail previews, folder unread counts, and folder kinds: synthesized design

Status: accepted design for issues #458, #459, and #460. [ADR 0067](../../decisions/0067-message-previews-folder-unread-counts-and-kinds.md) records the lasting decision.

Base: [candidate A](candidate-a.md). Read it in full; everything there holds unless changed below. The design came from a two-candidate architect arena. No cross-judge ran because only Claude models were configured.

## Owner decisions (2026-11-14)

- Store message previews. Previews are sanitized, capped at 256 characters, kept in the encrypted cache, and never logged or shown in Debug output. ADR 0067 amends ADR 0011's "do not request `bodyPreview`".
- Accept a one-time resync after the upgrade. The migration resets message checkpoints, so each folder rescans once and cached opened bodies are discarded once. They reload when a message is opened.
- EWS Archive follows Thunderbird's EWS client. That client is MPL-licensed, so we copy its behavior, never its code: `repos/thunderbird/rust/protocol_shared/src/lib.rs:47` and `ews_xpcom/src/client/sync_folder_hierarchy.rs:335`. Kinds resolve through one batched `GetFolder` with `BaseShape::IdOnly` over the distinguished ids `inbox`, `deleteditems`, `drafts`, `sentitems`, `junkemail`, and the undocumented `archive`. Thunderbird found that `archive` returns the account's archive folder, and that the documented `archive*` ids return folder-not-found. The client checks that the response count equals the request count and maps responses to ids by request order. Any single id that answers `ErrorFolderNotFound` or `ErrorSchemaValidation` is absent, so the folder shows as a normal folder. Unlike Thunderbird, we do not need the root folder, and kinds are cosmetic. Any other failure therefore leaves kinds unresolved and does not fail the sync (graft 2).

## Grafted from [candidate B](candidate-b.md)

1. **Kinds persist in the cache, not in a per-session cache.** `mail_folders.kind` is nullable. `NULL` means unresolved, `'other'` means resolved as a plain folder, and the remaining values are the six kinds. The workflow runs kind resolution after `replace_folders` only when some folder's kind is `NULL`. That happens on the first sync, after a new folder appears, and after the migration. So an app restart costs zero extra requests. This replaces A's `ProviderSession.folder_kinds` cache. A's `MailFolderKinds` one-to-one map and the unique index `mail_folders_one_per_kind ... WHERE kind IS NOT NULL AND kind <> 'other'` stay. `replace_folders` must not overwrite a stored kind with `NULL`. Only the resolution step writes kinds.
2. **Kind resolution never fails a sync.** Kinds are cosmetic. Any resolution error leaves kinds unresolved, the sync continues, and resolution retries on the next sync. A's 404-means-absent mapping still applies inside a successful resolution.
3. **One message column list.** The persistence PR adds a single `MESSAGE_COLUMNS` definition. Every message SQL statement is built from it: store, read metadata, read message/`RawMessage`, and the move insert. A's "every metadata field round-trips through store, list, read, and move" test guards it.

## Kept from A where B differed

- Unread count: A's single stored `unread_uncached`, computed at each accepted `Complete` commit as `MAX(0, unread_reported - cached_unread)`. Displayed unread is `unread_uncached` plus a live count of cached unread rows. No writer adjusts a count. B's baseline/anchor pair computes the same value with one more column.
- EWS counts come from the existing `SyncFolderHierarchy` listing. `collect_folders` starts with no sync state, so every listing is a full enumeration with fresh counts (verified in `exchange-ews/src/provider/mail.rs:298`). There is no extra `GetFolder` for counts and no `CachedOnly` label.
- `MailFolderKind` replaces `WellKnownMailFolder`, migrating every caller in one PR (move workflow, port, errors, router, desktop). There is no second kind list.
- One migration (entry 15) adds the columns, both indexes, `query_version = 3`, and the checkpoint reset.
- Not A's two-request EWS split. EWS kinds follow Thunderbird's single batched request; see Owner decisions.
- No EWS version gate for `item:Preview`. The transport already requests `Exchange2013_SP1`.
- Pinned order follows Wino: Inbox, Sent Items, Drafts, Archive, Deleted Items, Junk Email, then other folders in cache order. The badge caps at "999+". The accessible name is "Inbox, 12 unread".
- `MessagePreview` sanitizing follows A's rule (Cc and separators become a space, bidi controls and invisible padding are dropped, whitespace collapsed, trimmed, at most 256 chars, idempotent). The rule runs at both provider boundaries and on cache read. Record in `dependency-review.md` that no crate fits character classification of plain text.

## Implementation units

| # | Unit | Depends on |
|---|---|---|
| M1 | ADR 0067, including the ADR 0011 amendment, plus these design docs | none |
| M2 | Rename `WellKnownMailFolder` to domain `MailFolderKind` (six variants; Graph gains aliases, EWS stays unsupported for moves), add the domain types (`MessagePreview` with rule tests, `UnreadCount`, `MailFolderKinds`, `MailFolder` builders, `set_preview`). Providers pass `None`. No behavior change. | M1 |
| M3 | Persistence: migration 15, `MESSAGE_COLUMNS`, preview storage, the kind column with its write rules, `unread_reported` and `unread_uncached` with the derivation at `Complete`, folder ordering unchanged in SQL, workflow kind resolution step and the new `MailProvider::folder_kinds` port (default unsupported), migration, legacy-fixture and round-trip tests | M2 |
| M4 | Graph: `bodyPreview`, `unreadItemCount`, `folder_kinds` via six alias lookups, synthetic wire fixtures, request-count assertions | M3 |
| M5 | EWS: read `UnreadCount`, `item:Preview`, `folder_kinds` via one batched `GetFolder` over the six distinguished ids including `archive`, following Thunderbird (per-id `ErrorFolderNotFound` or `ErrorSchemaValidation` means absent; count and order checked), synthetic SOAP fixtures with a missing `archive` response | M3, parallel with M4 |
| M6 | Desktop: third row line (5.5rem rows), `NavItem` unread count, kind icons, pinned order, folder refresh after writes, preview scenario data, screenshots | M3; merges last |
