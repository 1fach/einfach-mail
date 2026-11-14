# Candidate A: derived unread counts, session-cached folder kinds, bounded previews

## Problem

Issues #458–#460 add a row preview line, folder unread counts, and folder kinds with icons in a pinned order. Each one touches the provider wire, domain, ports, the SQLCipher schema, and the desktop. The hard part is the count. The provider count is correct but refreshes only on a sync. The cache holds only 90 days (ADR 0011), so counting cached rows undercounts. The cache also has five kinds of `is_read` writer: confirmed writes, moves, delta upserts, delta removals, and rebuild reconciliation. A stored counter that each of them adjusts would drift the first time one forgets.

Constraints from Phase A:

- ADR 0011 says "Do not request `bodyPreview`". The new ADR has to amend that bullet explicitly.
- Stored Graph delta links carry the old projection inside an opaque token. `message_delta_reset_link` requires `$select == MESSAGE_SELECT` (url.rs:241–270). Once the constant changes, a 410 `Location` for an old cursor is rejected as `UnsafeLink`. A checkpoint reset is therefore required, not optional.
- **Correction to the grounding:** `collect_folders` (exchange-ews mail.rs:298) starts every listing with `sync_state = None`. Each EWS listing is a full hierarchy enumeration, returned as `Create` changes. `Folder::Folder.unread_count` is therefore fresh on every sync, and EWS counts do not go stale. Only `map_folder` has to stop dropping the count.
- `ews` 0.1.2: `Message.preview: Option<String>` is on the `Message` struct (common.rs:868, `rename_all = "PascalCase"`), and `RealItem` wraps `Message` for SyncFolderItems `Create` and `Update`. Requesting `item:Preview` therefore parses with no crate change. `GetFolder` exists (get_folder.rs) and returns one `ResponseClass` per requested id. `FolderShape` has only `base_shape`, with no additional properties. The transport sends `RequestServerVersion Exchange2013_SP1`, so `item:Preview` (Exchange 2013 schema) is valid on every server the app can reach.
- Thunderbird's EWS client (MPL, inspection only) resolves kinds with one `GetFolder` over distinguished ids. That list includes the undocumented `archive`. It treats `ErrorFolderNotFound` and `ErrorSchemaValidation` per message as "absent".

## Usage (caller's view)

The desktop reads only domain values from the cache. It does no count arithmetic.

```rust
// folder_navigation.rs: pinned kinds first, then cache order.
let folders = navigation_order(&mail_view.folders);
for folder in folders {
    NavItem::new(format!("folder:{}", folder.reference().folder_id().as_str()),
                 folder.display_name().to_owned(), folder_icon(folder.kind()))
        .unread_count(folder.unread_count().map(UnreadCount::get))
        .selected(selected);
}

// message_list.rs: the third row line.
let preview = message.metadata().preview().map(MessagePreview::as_str); // None => empty line
```

After any read-state or move outcome, the existing spawned task refreshes the folders from the single source:

```rust
let outcome = workflow.set_message_read_state(&folder, &id, requested).await;
let folders = workflow.list_cached_folders().await;          // new line in the task
this.update(cx, |view, cx| { view.finish_read_state_update(..); view.replace_folders(folders); });
```

A persistence test states the count contract:

```rust
cache.replace_folders(&acct, vec![inbox.with_unread_count(Some(UnreadCount::new(40)))]).await?;
cache.commit_delta_page(&inbox_ref, &state, complete_page /* 3 cached unread */, now).await?;
assert_eq!(unread(&cache).await, Some(40));                  // baseline = provider report
workflow.set_message_read_state(&inbox_ref, &m1, ReadState::Read).await?;
assert_eq!(unread(&cache).await, Some(39));
workflow.set_message_read_state(&inbox_ref, &m1, ReadState::Read).await?;
assert_eq!(unread(&cache).await, Some(39));                  // idempotent: no double count
```

Providers return complete folders from the existing `list_folders`. The workflow (`sync.rs`) does not change.

## Shape

### Domain types (`crates/domain/src/mail.rs`, with `mail/preview.rs` for the newtype)

```rust
/// Plain-text opening of a message body, as summarized by the provider.
/// Invariant: 1..=MAX_CHARS chars; no Cc, bidi-control, line/paragraph-separator or
/// invisible-padding chars; whitespace runs collapsed to one space; trimmed.
#[derive(Clone, Eq, PartialEq)]
pub struct MessagePreview(Box<str>);
impl MessagePreview {
    pub const MAX_CHARS: usize = 256;
    /// Normalizes untrusted provider or cached text. `None` when nothing printable remains.
    /// Idempotent: from_untrusted(p.as_str()) == Some(p).
    pub fn from_untrusted(text: &str) -> Option<Self> { todo!() }
    pub fn as_str(&self) -> &str { &self.0 }
}
// Debug prints "MessagePreview(<redacted>)".

/// The provider's or cache's best-known count of unread items in a folder.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct UnreadCount(u32);
impl UnreadCount { pub const fn new(n: u32) -> Self; pub const fn get(self) -> u32; }

/// A folder the mailbox designates for a role. Replaces application's WellKnownMailFolder.
#[derive(EnumSetType, Debug, Hash)]          // enumset already used by MailAction
pub enum MailFolderKind { Inbox, SentItems, Drafts, Archive, DeletedItems, JunkEmail }

/// Provider resolution of kinds to folder ids. One folder per kind and one kind per folder.
pub struct MailFolderKinds { /* HashMap<MailFolderKind, FolderId> */ }
impl MailFolderKinds {
    pub fn new() -> Self;
    /// Ignores the pair and returns false when either side is already assigned.
    pub fn assign(&mut self, kind: MailFolderKind, folder: FolderId) -> bool;
    pub fn kind_of(&self, folder: &FolderId) -> Option<MailFolderKind>;
    pub fn folder_of(&self, kind: MailFolderKind) -> Option<&FolderId>;
    /// Whether every assigned id appears in `listing`. False means re-resolve.
    pub fn covers(&self, listing: &[MailFolder]) -> bool;
}

impl MailFolder {           // new() unchanged: 17 call sites stay
    pub fn with_kind(self, kind: Option<MailFolderKind>) -> Self;
    pub fn with_unread_count(self, count: Option<UnreadCount>) -> Self;
    pub fn kind(&self) -> Option<MailFolderKind>;
    pub fn unread_count(&self) -> Option<UnreadCount>;
}
impl MessageMetadata { pub fn with_preview(self, p: Option<MessagePreview>) -> Self; pub fn preview(&self) -> Option<&MessagePreview>; }
impl MessageMetadataPatch { pub fn set_preview(&mut self, p: Option<MessagePreview>); }
```

**Preview rule.** `from_untrusted` handles each char in turn:

- Cc characters, U+2028 and U+2029, and Unicode whitespace become a single space.
- Bidi controls are dropped: U+061C, U+200E–U+200F, U+202A–U+202E, and U+2066–U+2069. They prevent Trojan-Source-style reordering in a list row.
- Invisible preheader padding is dropped: U+00AD, U+034F, U+200B, U+200C, U+2060, and U+FEFF. Marketing mail pads its previews with these.
- The result is trimmed, and the function stops after 256 output chars, so its work is bounded.

There is no HTML decoding, because both providers send plain text. Record in `dependency-review.md` that no crate fits: this is char classification, not parsing or sanitization of markup. The cache read path runs the same function, which is idempotent, so saved data is revalidated without a second rule. Each validation lives at one boundary (boundary-discipline).

**Patch semantics.** The field is `preview: FieldUpdate<Option<MessagePreview>>`. `into_complete_message` treats `Unchanged` as `None`, as it already does for the flag, so preview never gates insertion.

- Graph `WireField`: `Missing` changes nothing. `Null` sets `None`. `Value(s)` sets `from_untrusted(&s)`, so an all-whitespace preview clears the field.
- EWS `Create` and `Update` carry the full shape, so they always set `message.preview.as_deref().and_then(from_untrusted)`. `ReadFlagChange` leaves the field untouched.

**Folder kind.** The variant order is not presentation. The pinned order lives in the desktop. `WellKnownMailFolder` is deleted and every use becomes `MailFolderKind`: the move workflow, the `well_known_folder` port, the `MailWorkflowError` variants, the account router, and the desktop. Keeping both would mean two lists that must stay in sync (red flag). Both adapters use `MailFolderKinds`, so one-to-one assignment is enforced in one place.

### Unread count: derived, never maintained

`mail_folders` stores two numbers:

- `unread_reported`: the latest provider count, written by `replace_folders`.
- `unread_uncached`: `MAX(0, unread_reported − cached_unread)`, computed only inside the `MessageDeltaProgress::Complete` branch of `update_sync_progress`. It runs after `reconcile_full_sync` and in the same transaction. This is the number of unread items the cache cannot see: older than 90 days, or of item classes it filters out.

`read_folders` returns the following. `NULL` becomes `None`, which shows no badge.

```sql
CASE WHEN f.unread_uncached IS NULL THEN NULL
     ELSE f.unread_uncached + (SELECT COUNT(*) FROM mail_messages m
          WHERE m.account_id = f.account_id AND m.folder_id = f.folder_id AND m.is_read = 0)
END
```

Every `is_read` writer is already correct, with no new code:

| Event | Effect |
|---|---|
| Confirmed read or unread (`set_message_read_state`) | The row's `is_read` changes and the count follows. Repeating the write is a no-op, so nothing is counted twice. |
| `ConfirmedNeedsSync` (cache write failed) | The row did not change, so the count did not change either. The folder write marker remains, and the next completed round re-baselines. The count can never drift from the cached rows. |
| Move or delete (`move_cached_message`) | The source loses the row and the destination gains it. On a destination conflict the destination's `is_read` is kept, so it is counted once. The later deltas (source removal, destination upsert) are no-ops. |
| Delta upsert, delta removal, or cutoff exclusion | The row changes, and the count follows. |
| Rebuild (ADR 0032) | Unseen rows survive until the final page. The reconcile then deletes them, and the re-baseline runs after that. |
| Stale page (ADR 0037) | The page is rejected, so no re-baseline runs. |

**Refresh rule.** The count re-baselines at each accepted `Complete` commit, once per folder per successful sync. Arrivals between `list_folders` and that folder's completion are briefly under-reported. The error is bounded by one sync's arrivals, and the clamp at 0 and the next sync correct it. Reads by other clients on mail older than 90 days are also corrected at the next completion. A partial index, `mail_messages_unread (account_id, folder_id) WHERE is_read = 0`, keeps the subquery cheap.

Interface depth: neither port gains a method. The policy hides behind `replace_folders`, `commit_delta_page`, and `list_folders`, and their rustdoc states it. Rows own read state, and the folder row owns only the provider-reported numbers.

### Provider work

**Graph**

- `FOLDER_SELECT = "id,displayName,isHidden,unreadItemCount"`. The wire field is `Option<i64>` (`serde(default)`). The mapping is `u32::try_from(n).ok().map(UnreadCount::new)`: a malformed count means unknown and never fails the sync.
- `MESSAGE_SELECT` adds `bodyPreview`, and the wire field is `WireField<String>`.
- Kinds: `well_known_folder_path(kind)` gains the aliases `inbox`, `sentitems`, `drafts`, `junkemail`, `deleteditems`, and `archive` in an exhaustive `match`. Add `GraphReadOperation::WellKnownFolder`, which maps 404 to `Ok(None)` (absent kind). Every other error propagates, as today.
- `ProviderSession` gains `folder_kinds: Option<MailFolderKinds>`. `list_folders` traverses the tree first. It then resolves the kinds when none are cached or when `!kinds.covers(&folders)`, and applies `with_kind(kinds.kind_of(id))`. An id that is not in the visible listing, such as a hidden folder, gets no kind.
- `well_known_folder()` for moves reads the same cache and issues a GET only on a miss.
- Cost: 6 GETs on the first sync of a session or after a resolved id disappears, and 0 on later syncs. Counts and previews ride on requests that already happen. `$batch` would reduce 6 to 1 but adds a new response parser, so it is deferred.

**EWS**

- `map_folder` reads `Folder::Folder { unread_count, .. }`. Other variants get `None`.
- `message_metadata_shape()` adds `item:Preview`.
- Kinds: two `GetFolder` requests with `BaseShape::IdOnly`:
  - The first covers the documented `inbox`, `sentitems`, `drafts`, `junkemail`, and `deleteditems`. It requires exactly 5 response messages in request order. `ErrorFolderNotFound` means absent. Any other error propagates.
  - The second covers the undocumented `archive` on its own. Any SOAP fault or error response means absent, so it cannot fail the sync.
- Session-cached with the same `covers` rule. `well_known_folder()` stays `UnsupportedOperation`, because moves are not EWS capabilities (ADR 0058).
- Cost: 2 requests per session. No `MailCapabilities` change: all three fields are reads, and a missing value degrades to `None`.

### Migration (entry 15, appended)

```sql
ALTER TABLE mail_messages ADD COLUMN preview TEXT
  CHECK (preview IS NULL OR length(preview) BETWEEN 1 AND 256);
ALTER TABLE mail_folders ADD COLUMN kind TEXT CHECK (kind IS NULL OR kind IN
  ('inbox','sent_items','drafts','archive','deleted_items','junk_email'));
ALTER TABLE mail_folders ADD COLUMN unread_reported INTEGER
  CHECK (unread_reported IS NULL OR unread_reported >= 0);
ALTER TABLE mail_folders ADD COLUMN unread_uncached INTEGER
  CHECK (unread_uncached IS NULL OR unread_uncached >= 0);
CREATE UNIQUE INDEX mail_folders_one_per_kind ON mail_folders (account_id, kind) WHERE kind IS NOT NULL;
CREATE INDEX mail_messages_unread ON mail_messages (account_id, folder_id) WHERE is_read = 0;
UPDATE mail_account_sync_scope SET query_version = 3;
UPDATE mail_sync_state SET delta_link = NULL, next_link = NULL, last_sync_seconds = NULL,
  last_sync_nanoseconds = NULL, full_sync_generation = 0, full_sync_active = 0,
  checkpoint_revision = checkpoint_revision + 1;
UPDATE mail_messages SET full_sync_generation = NULL;
```

- **Backfill.** Reset the checkpoint, as the flag migration did, and set `MESSAGE_QUERY_VERSION = 3`. Graph requires the reset. EWS would otherwise never preview old items. The cost is one metadata scan per folder, and the upserts clear cached opened bodies (ADR 0034).
- `validate_folder_snapshot` also rejects two folders with the same kind.
- `insert_folder` writes `kind`, `unread_reported` and the `ON CONFLICT` update. It does not touch `unread_uncached`.
- Every message column list gains `preview`: `store_message_metadata`, `read_cached_metadata`, `read_cached_message`/`RawMessage`, and `MOVE_CACHED_MESSAGE_SQL` with `preview = COALESCE(mail_messages.preview, excluded.preview)`.
- **Versions 5 and 7.** The compatibility shims run first. The v5 repair sets `query_version = 2`, and entry 15 then sets it to 3. The v7 repair stamps 8 and continues. No new shim is needed because entry 15 has no parallel twin. Land it alone so that no other branch claims index 15.

**Tests** (`schema/tests.rs`)

- Update `assert_flag_transition_preserves_cached_mail` to expect `MESSAGE_QUERY_VERSION`. It currently hard-codes 2 after `to_latest`.
- A new test migrates to 14, inserts a synthetic folder, scope, sync state, and message with a body, migrates to latest, and asserts that the rows survive, the new columns are `NULL`, `query_version = 3`, the links are cleared, and the revision has increased.
- Constraint tests: a 257-char preview, an unknown kind, a negative count, and two inboxes are each rejected.
- The v5-parallel and v7-reply tests reach latest.
- The existing legacy `legacy-sqlcipher-4.14-v1.db` production test still opens.
- Add a store-level "every metadata field round-trips" test: store, list, read, and move a fully populated message. Adding a column to one SQL list and missing another then fails a test instead of dropping data silently (hand-synced list red flag).

### UI

**Message rows**

- Add a third line in `render_message_row_content`: `text_xs`, `muted_foreground`, truncated, with the `message-preview-{id}` debug selector.
- `MESSAGE_ROW_HEIGHT` goes from `rems(4.5)` to `rems(5.5)`, and `day_groups.rs` picks the change up automatically.
- The height stays constant whether or not a preview exists. The line renders empty, so the virtual list keeps uniform sizes and an even rhythm.
- `MessageRowSummary` gains `preview: Option<String>`. `accessible_details()` appends it after "Flagged". The name stays "sender. subject".

**Folder navigation**

- `NavItem::unread_count(Option<u32>)` renders a trailing muted `text_xs` count when it is above 0, capped at "999+". It also changes the accessible label to "Inbox, 12 unread". The `folder:{id}` ids and focus order are unchanged.
- `folder_icon(Option<MailFolderKind>)` is an exhaustive `match`:

  | Kind | `AppIcon` | Lucide icon |
  |---|---|---|
  | Inbox | `Inbox` | inbox |
  | SentItems | `SentItems` | send |
  | Drafts | `Drafts` | file-pen |
  | Archive | `Archive` (existing) | archive |
  | DeletedItems | `DeletedItems` | trash |
  | JunkEmail | `JunkEmail` | shield-alert |
  | None | `Folder` | folder |

  All of these exist in gpui-kit-assets 0.9.1, so they are new rows in `app_icons!`.
- `PINNED_FOLDER_ORDER: [MailFolderKind; 6]` uses Wino's order: Inbox, Sent Items, Drafts, Archive, Deleted Items, Junk Email. `navigation_order` stable-sorts by pinned rank and keeps cache order for the rest. A test asserts that the array contains `EnumSet::all()` exactly once each.
- After a read or move task, call `list_cached_folders`. The pane already observes `MailView`.
- The preview scenario (`app/preview.rs`) gains synthetic previews, counts, and kinds for screenshots.

## Decomposition

1. **ADR 0067 and the ADR 0011 amendment** (docs). Blocking.
2. **Rename `WellKnownMailFolder` to domain `MailFolderKind` with six variants.** Graph aliases are added; EWS stays unsupported; the router and desktop are renamed. No behavior change, so the existing move tests pass. Blocking for 3.
3. **Domain decorations**: `MessagePreview` (with unit tests for the rules and idempotence), `UnreadCount`, `MailFolderKinds`, the builders, and `set_preview`. Providers still pass `None`. Blocking for 4–7.
4. **Persistence migration 15** with storage, derivation, the round-trip test, and the migration tests. Blocking for 5, because the Graph select change must not reach users before the checkpoint reset.
5. **Graph provider** (select changes, kinds with the session cache, 404 mapping, synthetic wire and transport fixtures, request-count assertions). Runs in parallel with 6, after 4.
6. **EWS provider** (`UnreadCount`, `item:Preview`, two `GetFolder` requests, synthetic SOAP fixtures including an `archive` fault). Runs in parallel with 5, after 4.
7. **Desktop** (row line and height, `NavItem` count, icons, pinned order, folder refresh after writes, headless tests for the label, order, height, and refresh, and the preview screenshots). It can be built in parallel with 4–6 against preview data, and it merges last.

Each PR runs `scripts/check-linux-local.sh`. PRs 5 and 6 need recorded, sanitized fixtures before anyone claims live behavior.

## ADR 0067 outline: message previews, folder unread counts, and folder kinds

- **Context:** #458–#460; the ADR 0011 ban on `bodyPreview`; the 90-day cache; the delta select constraint; EWS hierarchy listings are full enumerations.
- **Decision:**
  1. `MessagePreview` and its normalization rule, applied at both provider boundaries and on cache read.
  2. Request Graph `bodyPreview` and EWS `item:Preview`.
  3. The unread count is derived as the provider report plus the cached unread rows, re-baselined at each completed delta round. No writer adjusts it.
  4. `MailFolderKind` replaces `WellKnownMailFolder`. Graph resolves aliases and EWS uses `GetFolder` on distinguished ids, both cached per provider session.
  5. Migration 15 with a checkpoint reset and `query_version` 3.
  6. Desktop pinned order, icons, the count, and the third line.
- **Amends** the ADR 0011 metadata bullets (adds preview, kind, and unread count). **Relates to** ADRs 0032, 0034, 0037, 0058, and 0066.
- **Consequences:** a full metadata rescan and loss of cached opened bodies once after the upgrade; more sensitive text at rest (encrypted); 6 Graph and 2 EWS requests per session; rows 22% taller.

## Synthesis decision

The arena chose this candidate as the base. [The synthesis](synthesis.md) records the grafts and the owner decisions that change it.

## Tradeoffs accepted

- We accept a count error during one sync (arrivals between listing and completion) in exchange for a count that cannot drift from cached read state and that needs no new code on any `is_read` writer.
- We accept a full metadata rescan and losing offline bodies after the upgrade, in exchange for previews on existing mail and Graph cursors that stay valid.
- We accept a constant 5.5 rem row with an empty third line when there is no preview, in exchange for uniform virtual-list sizing.
- We accept per-session kind caching, so an Archive folder created mid-session appears after a restart, in exchange for no extra requests on routine syncs.
- We accept writing a reported count and reading back a derived one through the same `MailFolder` field. The port rustdoc states this.

## Alternatives considered

- **A stored counter adjusted by transition-aware SQL.** It needs ±1 logic in six writers of one number (split ownership). One missed path drifts silently, and `ConfirmedNeedsSync` needs special handling.
- **The workflow adjusts the count** (the issue's wording). It needs the prior state, ignores moves and deltas, and leaks cache policy into application code.
- **Cached rows only** undercounts old unread mail. **Provider count only** ignores the user's own reads until the next sync.
- **Kinds by display name or Graph `wellKnownName`.** Display names are localized, and `wellKnownName` is not on the v1.0 `mailFolder`.
- **Sanitize the preview at display time.** This spreads the rule to every reader.

## Open questions and risks

- Does EWS `SyncFolderHierarchy` with `BaseShape::Default` return `UnreadCount`? Microsoft's examples suggest it does, and `FolderShape` cannot request it explicitly. If a recorded response shows it missing, should the fallback be a `GetFolder` with `AllProperties` in batches of 10?
- The EWS `archive` id is undocumented (Thunderbird relies on it). Is "absent on any fault" acceptable, or should EWS Archive simply be out of scope?
- Does the owner accept storing 256 characters of every message body (encrypted) when ADR 0011 originally excluded previews?
- The one-time loss of cached opened bodies after the upgrade: acceptable, or should migration 15 exempt preview-only upserts from ADR 0034's body clearing?
- Should the count be hidden for Sent Items and Deleted Items, as Outlook does, or shown everywhere, as Wino does? This candidate shows it everywhere.
- Plain trailing count text or a GPUI Kit `Badge`? gpui-component's `Badge` is an overlay. This candidate uses inline text; confirm with the gpui-kit-design-guides skill.
- Stripping U+200C affects Persian and Indic shaping in previews. Is that acceptable for a single truncated line?

## Next implementation step

Write ADR 0067 and the ADR 0011 amendment, then do the `WellKnownMailFolder` to `MailFolderKind` rename as a no-behavior PR.
