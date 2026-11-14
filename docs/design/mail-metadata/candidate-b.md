# Candidate B: derived unread counts, resolved-once folder kinds, one inert schema migration

## Problem

Wino-style rows and folders need three provider-fed facts: a preview per message (#458), an unread count per folder (#459) and a folder kind for icon and order (#460). They cross the Graph and EWS wire, domain, the `MailCache` port, SQLCipher and GPUI. The constraints that shape the design:

- ADR 0011 says "Do not request `bodyPreview`". The new ADR (proposed 0067) must amend that bullet. Previews put about 255 characters of body text for every 90-day message into the cache.
- Only a 90-day window is cached (ADR 0011), so counting cached rows undercounts. The provider count is right but is only fetched at `list_folders`.
- Graph `mailFolder` v1.0 has no well-known name. EWS hierarchy sync has no distinguished id and does not reliably report unread counts.
- The read-state write path never reads the previous `is_read`. Moves and delta removals also change counts. Any counter that adjusts on transitions has to get all of those paths right.
- Migrations are append-only. Branches have already collided (the `compatibility.rs` repairs for versions 5 and 7). A migration list that three parallel PRs all append to would collide again.

Verified in source: `ews` 0.1.2 `Message.preview: Option<String>` is a serde field, so SyncFolderItems parsing fills it when the shape asks for `item:Preview`. `GetFolder` is typed (`FolderShape`, `Vec<BaseFolderId>`) and batches many ids in one request. `Folder::Folder.unread_count` exists.

## Usage (caller's view)

Desktop folder pane (the only reader of kind and unread):

```rust
let entries: Vec<FolderEntry> = workflow.folder_entries().await?; // already in pinned order
for e in &entries {
    NavItem::new(format!("folder:{}", e.id()), e.display_name(), folder_icon(e.kind()))
        .badge(e.unread().map(UnreadBadge::from))   // None => no badge
        .accessible_name(e.accessible_name());        // "Inbox, 12 unread"
}
```

Message row (`MessageRowSummary`):

```rust
let preview: Option<&str> = message.metadata().preview().map(MessagePreview::as_str);
```

Provider (Graph delta mapping), where preview is lenient and never fails a page:

```rust
match wire.body_preview {                        // WireField: Missing | Null | Value
    WireField::Missing => {}                     // patch.preview stays Unchanged
    WireField::Null => patch.set_preview(None),
    WireField::Value(raw) => patch.set_preview(MessagePreview::from_provider_text(&raw)),
}
```

After a read-state write the view only re-reads `folder_entries()`. It does no count arithmetic.

## Shape

### Domain (`crates/domain/src/mail.rs`)

```rust
/// Plain text, <= 256 chars, no control/format/bidi chars, whitespace runs collapsed.
#[derive(Clone, Eq, PartialEq)] pub struct MessagePreview(String);   // Debug redacted
impl MessagePreview {
    pub const MAX_CHARS: usize = 256;
    /// Lenient: returns None for empty-after-sanitizing input. Never errors.
    pub fn from_provider_text(raw: &str) -> Option<Self>;
    pub fn as_str(&self) -> &str;
}
```

- `from_provider_text` maps Cc, U+200B-200F, U+202A-202E, U+2060-2069 and U+FEFF to a space. It collapses whitespace runs, trims, and truncates at a `char` boundary (so at most 1,024 bytes). An empty result is `None`.
- `MessageMetadataPatch` gains `preview: FieldUpdate<Option<MessagePreview>>`. Missing maps to `Unchanged`, Null to `Set(None)` and Value to `Set(Some)`. `MessageMetadata` gains `preview: Option<MessagePreview>`. `into_complete_message` does not require it.
- `pub enum MailFolderKind { Inbox, Sent, Drafts, Junk, Deleted, Archive }` has no `Other` variant, because "not special" is `Option::None`. It owns `PINNED_ORDER: [MailFolderKind; 6]`, which is the only order list.
- `pub struct UnreadCount(u32)` has a lenient `from_wire(i64) -> Option<Self>` (negative gives `None`, large values saturate).
- `MailFolder` gains `unread_reported: Option<UnreadCount>`. It is provider-reported at list time and is not the displayed value. Debug redaction is unchanged.
- `pub enum FolderUnread { Provider(UnreadCount), CachedOnly(UnreadCount) }`, defined in application with `FolderEntry`. `CachedOnly` means "counted from the 90-day cache, may undercount". It is honest labeling instead of silent drift.

### Application

- `FolderEntry { folder: MailFolder, kind: Option<MailFolderKind>, unread: Option<FolderUnread> }` is a read model returned by `MailCache::folder_entries`. It is sorted by `fn navigation_order(&mut [FolderEntry])`, the only place pinned order is applied. Pinned kinds come first in `PINNED_ORDER`. Then come the rest by name and id.
- The existing `WellKnownMailFolder` (move targets: Archive and DeletedItems) is kept. It gains `fn kind(self) -> MailFolderKind`, so there is one alias table keyed by `MailFolderKind`. This deviates from the issue's "extends": the kind enum supersedes by composition. Folding the move enum into `MailFolderKind` would force a runtime check against moving into Sent or Drafts.
- New port method `MailProvider::folder_kinds(&self, &AccountId) -> Result<Vec<(FolderId, MailFolderKind)>, MailOperationError>`. It is one call with the batching hidden, and it is separate from `well_known_folder`, whose EWS `UnsupportedOperation` is a move-capability fact (ADR 0058). A kind lookup is a read.
- `synchronize_account` runs: `list_folders`, `replace_folders`, then `if cache.has_unresolved_folder_kinds() { provider.folder_kinds() -> cache.store_folder_kinds() }`, then the delta loops. A resolution failure is swallowed (kinds stay unresolved and are retried on the next sync). It never fails the sync, because it is cosmetic.

### Persistence: one inert-columns migration (#15), one data migration (#16)

Migration 15 only adds nullable columns:

- `mail_messages.preview TEXT CHECK (preview IS NULL OR length(preview) <= 256)`.
- `mail_folders.folder_kind TEXT CHECK (folder_kind IS NULL OR folder_kind IN ('inbox','sent','drafts','junk','deleted','archive','other'))`. NULL means unresolved and `'other'` means resolved as plain. Re-resolution is needed exactly when any folder's kind is NULL, so a newly created folder triggers it and no timer is needed.
- `mail_folders.unread_reported`, `unread_baseline` and `unread_anchor`, all `INTEGER NULL CHECK (>= 0)`.
- Partial index `mail_messages_unread ON mail_messages (account_id, folder_id) WHERE is_read = 0`.

Why land every column up front (the inert-schema PR): it keeps migration numbering conflict-free while the feature PRs run in parallel. The columns are nullable and unread until their feature lands.

`replace_folders` upserts name, parent and `unread_reported`. It never touches `folder_kind`, `unread_baseline` or `unread_anchor`. The folder SQL column lists (`insert_folder`, `read_folders`) and the message lists (`store_message_metadata`, `read_cached_metadata`, `RawMessage`, `MOVE_CACHED_MESSAGE_SQL`) are today's hand-synced lists. The preview PR introduces one `MESSAGE_COLUMNS` const (and a folder equivalent) that all of them format from. A test asserts the move SQL copies every column in it.

## Unread count semantics: derive, do not adjust

Displayed unread = `MAX(0, unread_baseline + local_unread_now - unread_anchor)`, computed in the `folder_entries` SQL. `local_unread_now` is `COUNT(*) WHERE is_read = 0` via the partial index.

- `unread_baseline` and `unread_anchor` are written together, in the same transaction as the final delta page (`Complete`). That transaction runs after the unseen-message deletions of an ADR 0032 rebuild. It sets `unread_baseline = unread_reported` and `unread_anchor = COUNT(unread rows)`. A stale page is rejected by the ADR 0037 revision check, so a stale sync cannot promote.
- Between syncs, any change to the rows moves the count with them: confirmed read or unread (`set_message_read_state`), a local move (source down, destination up), and delta removals and read-state updates. There is no transition tracking and no previous `is_read` to read, so double counting is impossible by construction. The count is a pure function of the rows plus two numbers.
- Out-of-window unread mail is in `unread_baseline` and unaffected by local drift, which is right (the cache never holds those rows). Read-state deltas for rows not in the cache are not applied, which is the existing ADR 0011 rule. The next promotion absorbs them.
- Refresh rule: every completed folder round overwrites the baseline and anchor. A folder that never completes keeps its last pair, so counts degrade gradually.
- `ConfirmedNeedsSync` (provider confirmed, cache write failed): the rows did not change, so the displayed count does not change. This is a consistent, honest cache view. The existing write-sync marker forces a delta round, which flips the row and promotes a fresh pair. No compensation code exists, so none can be wrong. A UI toast already says the cache needs a sync.
- First sync: `unread_baseline` is NULL, so the badge is hidden (`unread: None`) until the folder's first complete round. A provider that supplies no counts (an EWS failure of the count request) gets `FolderUnread::CachedOnly(local)` after the first complete round.
- Known imprecision: mail that arrives between `list_folders` and a folder's completion is in the anchor but not the baseline, so it undercounts until the next sync. The error is bounded by one sync duration and self-heals.
- The folder pane must re-read `folder_entries` after any read-state write, move, or sync. This is the view reload that is missing today.

## Provider work

### Graph

- `FOLDER_SELECT += ,unreadItemCount` and `MESSAGE_SELECT += ,bodyPreview`. Zero extra requests for preview or counts.
- Kinds: `folder_kinds` issues `GET /me/mailFolders/{alias}?$select=id` for inbox, sentitems, drafts, junkemail, deleteditems and archive, reusing the `well_known_folder_path` pattern. That is 6 requests, but only when a folder kind is unresolved (the first sync and each new folder), so about 0 amortized. A 404 on one alias (for example no archive) means "none", not an error. Any other failure aborts resolution and retries next sync. A `$batch` call would make it 1 request, but it is a new transport pattern, so it is deferred.
- `MESSAGE_SELECT` is encoded into stored delta links. Old links never return `bodyPreview`, so the preview PR must reset checkpoints (see Migration). `message_delta_reset_link` already compares `$select == MESSAGE_SELECT`, so it follows the constant. Test: a reset Location with the old select is rejected and one with the new select is accepted.

### EWS

- Preview: add `item:Preview` to `message_metadata_shape()` and map `Message.preview` through `from_provider_text`. Gate it on the server version being >= Exchange 2013 (15.0). ADR 0058 already reads `ServerVersionInfo` at connect. Requesting an unknown FieldURI from an older server could fail the whole sync, so an older server omits the property and previews stay `None`.
- Kinds: `folder_kinds` is one `GetFolder` with `DistinguishedFolderId` for inbox, sentitems, drafts, junkemail, deleteditems and archive, `IdOnly`. The returned `FolderId`s are matched to the hierarchy ids (1 extra request, cached like Graph). The `ews` crate surfaces a per-id error response such as a missing archive in a way I have not verified, so a fixture is required. The existing `well_known_folder` stays `UnsupportedOperation` for moves.
- Unread: do not trust `SyncFolderHierarchy`'s `unread_count` (stale and incremental, so `map_folder` keeps ignoring it). After hierarchy sync, send one batched `GetFolder` by folder id with `folder:UnreadCount`, chunked to 100 ids. That is +1 request per sync for typical mailboxes, and `unread_reported` comes only from there. If it fails, counts are `None` and the UI shows `CachedOnly`. This is the single source of truth for EWS counts.
- Capability: previews, kinds and counts are read-side, so no `MailAction` is added.

## Migration

- #15 (columns, in the schema PR): `ALTER TABLE ... ADD COLUMN` for each column above, plus the index. `ADD COLUMN` with a CHECK and a NULL default is valid. It needs no `compatibility.rs` change, because #15 runs only from head 14. Parallel-branch DBs at version 5 or 7 are repaired by existing code and then walk migrations 6 to 15 normally.
- #16 (data-only, ships in the preview PR together with the `MESSAGE_SELECT` change): precedent is `FLAG_SCHEMA_TRANSITION_SQL`. It sets `UPDATE mail_account_sync_scope SET query_version = 3` (and `MESSAGE_QUERY_VERSION = 3`, whose mismatch otherwise returns `InvalidData`) and resets `mail_sync_state` (links NULL, generation 0, `full_sync_active = 0`), and clears `full_sync_generation` on messages. Rows stay visible while the one-time refetch runs. The refetch costs one full unfiltered delta per folder, which ADR 0011 already accepts for first sync.
- Backfill: only preview needs a checkpoint reset. Kinds and unread need none (NULL columns self-resolve on the next sync). Keeping the reset out of #15 avoids a wasted full refetch before `bodyPreview` is even requested.
- Tests in `shared/schema/tests.rs`:
  - Migrate the `legacy-sqlcipher-4.14-v1.db` fixture to head. Assert it opens, old rows are intact, and the new columns are NULL.
  - Extend `parallel_send_v5_schema_receives_the_missing_flag_migration` and the v7 reply tests to assert the new columns after migration.
  - A "#16 resets checkpoints and keeps rows" test, mirroring `flag_migration_preserves_cached_mail_and_resets_message_checkpoints`.
  - A `CHECK` violation test for an oversize preview.

## UI

- Row: a third line (`text_xs`, `muted_foreground`, `truncate`, single line) under the subject. `MESSAGE_ROW_HEIGHT` goes from 4.5rem to 5.5rem (the design doc value), and `day_groups.rs` keeps using the token. The line's height is reserved even when preview is `None`, so the virtual list heights stay fixed. Test ids are `message-preview-{id}`. The preview is plain text only, with no markup or link handling, and it is not part of the accessible label. The label stays "sender, subject, time, unread". The preview goes into the details/description. ADR 0024 keyboard and ListBox behavior is unchanged.
- Folders: `NavItem::trailing_badge(Option<Badge>)`. Visual cap is "99+", while the accessible name carries the exact value: "Inbox, 12 unread". A `CachedOnly` value reads "12 unread in cached mail". Badge test id is `folder-badge-{id}`. The `nav-*` and `folder:{id}` ids are untouched.
- Icons: add `AppIcon::{Inbox, Send, FilePen, ShieldAlert, Trash2, Archive}` to the `app_icons!` table. One exhaustive `fn folder_icon(Option<MailFolderKind>) -> AppIcon` match in `folder_navigation.rs`. Pinned order comes from the domain, so the view does no sorting. Order: Inbox, Sent, Drafts, Junk, Deleted, Archive, then other folders by name.

## Decomposition

0. **ADR 0067** (blocking; docs only). It amends the ADR 0011 bullet and carries the privacy decision on stored previews.
1. **Domain, ports and inert schema (migration #15)** (blocking). `MessagePreview`, `MailFolderKind`, `UnreadCount`, `FolderEntry`, `folder_kinds`, `folder_entries` and the legacy-fixture, v5 and v7 tests. All provider implementations return `None` or `Unsupported`.
2. **Preview** (after 1; parallel with 3 and 4). Graph select and wire, EWS shape and version gate, persistence writes and reads with `MESSAGE_COLUMNS`, migration #16, and fixtures. 2b is the row UI (parallel with 2a once the domain type exists).
3. **Kinds** (after 1; parallel with 2 and 4). Graph resolver, EWS `GetFolder`, `replace_folders` merge rules, workflow resolution, `navigation_order`, and then 3b (the `AppIcon`s and order). 3a: backend.
4. **Unread** (after 1). 4a: Graph select, final-page promotion SQL, `folder_entries`, and the view reload. 4b: the badge in `NavItem`. 4c: the EWS batched `GetFolder`. 4c depends on the EWS `GetFolder` plumbing from 3a.
5. Preview-scenario screenshots, `status.md` and roadmap updates.

PR2, PR3 and PR4 touch `replace_folders` and the folder SQL lists, so expect small rebase conflicts but no migration collision.

## Synthesis decision

The grafting candidates were the derived-count formula and the NULL-means-unresolved kind column. [The synthesis](synthesis.md) records what the arena took from this candidate.

## Tradeoffs accepted

- We accept three columns' worth of inert schema for a few PRs, in exchange for no migration-number collisions.
- We accept one extra SQL count per folder read (partial index) in exchange for no transition-tracking code anywhere.
- We accept up to six extra Graph requests on first sync and for each new folder, in exchange for no per-sync overhead and no name heuristics.
- We accept `unread_reported` on `MailFolder` even though the UI never reads it, in exchange for keeping the provider boundary a plain `list_folders`.
- We accept a one-time full metadata refetch for preview. It is the same cost as first sync, and rows stay visible.
- We accept lenient parsing of preview and counts. A malformed value means `None`, never a failed sync, since both are cosmetic.

## Alternatives considered

- Provider counts only, refreshed at `list_folders`. Interface: trivial. Loses: counts are stale after every read in the app (Wino-like UX needs instant decrement) and the UI has to patch them.
- Local-only "cached unread". Interface: the simplest, with zero provider work. Loses: undercounts for any mailbox with old unread mail, which will look broken next to Outlook. Kept only as the `CachedOnly` fallback.
- Baseline plus transition-aware adjustment in the workflow (`set_message_read_state` reads the previous `is_read`, and moves and deltas each adjust). Spreads count rules across four write paths and `ConfirmedNeedsSync` needs compensation. Rejected under split ownership.
- Folder kind resolved at read time by display-name matching. Wrong for localized names. Rejected.
- Kind resolved in `list_folders` on every sync. Hides cost in the common path and bloats the most-called port.
- Three ADRs and three independent migrations. Cheaper reviews, but three schema histories for one design, and the three PRs would conflict on `replace_folders`.
- A separate `mail_message_preview` table. Avoids one column but duplicates the move, delta and delete writes.

## Open questions and risks

- Privacy: the cache will hold ~255 characters of body text for every message within the 90-day window, where today it holds only opened bodies. Is that acceptable for an encrypted cache, or should there be an account setting that stops storing previews (not just hiding them)?
- Pinned order: the issue lists Inbox, Sent, Drafts, Junk and the grounding adds Deleted and Archive. Is "Inbox, Sent, Drafts, Junk, Deleted, Archive" what the owner wants?
- Does the `ews` crate return the other responses in a multi-id `GetFolder` when one id errors (for example archive on a server without it)? A recorded fixture needs to confirm this, or resolution needs one id per request.
- Is a small hard-coded bidi and format denylist acceptable, or should `MessagePreview` use a maintained Unicode general-category crate? Needs a `dependency-review.md` entry either way (AGENTS says library first).
- Is hiding the badge until first promotion acceptable, or should the first sync show a rising `CachedOnly` number?
- Graph batch: should the six alias lookups use JSON `$batch` in a follow-up?

## Next implementation step

Write ADR 0067, then PR1: `MessagePreview` with its sanitizing tests and `MessageMetadataPatch::set_preview`, plus migration #15 with the legacy-fixture, v5 and v7 tests.
