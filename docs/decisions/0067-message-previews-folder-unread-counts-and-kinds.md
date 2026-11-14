# ADR 0067: Show message previews, folder unread counts, and folder kinds

- Status: Accepted; amends the message and folder metadata bullets of [ADR 0011](0011-initial-mail-sync-scope.md); implementation tracked in issues #458–#460
- Date: 2026-11-14
- Owners: Einfach Mail maintainers

## Context

The repository owner wants message rows and folders to look like Wino Mail. Each message row shows a preview line ([#458](https://github.com/1fach/einfach-mail/issues/458)). Each folder shows its unread count ([#459](https://github.com/1fach/einfach-mail/issues/459)). Inbox, Sent Items, Drafts, Archive, Deleted Items, and Junk Email show their own icons in a pinned order ([#460](https://github.com/1fach/einfach-mail/issues/460)). [ADR 0066](0066-fluent-visual-language-and-navigation.md) restyled the workspace with existing data and left these three additions to a separate decision. All three cross the same layers: the Graph and EWS wire, the domain, the application ports, the SQLCipher schema, and the desktop. One decision covers them.

Constraints from the code and from earlier decisions:

- [ADR 0011](0011-initial-mail-sync-scope.md) says "Do not request `bodyPreview`" in folder listing and delta responses. A preview needs that rule changed.
- The cache holds only messages from the last 90 days (ADR 0011). Counting cached unread rows undercounts any folder with older unread mail. The provider count is correct, but it refreshes only when the folders are listed during a sync.
- Five kinds of writer change a cached message's `is_read`: a confirmed read-state write, a move, a delta upsert, a delta removal, and the reconcile step of a replacement scan ([ADR 0032](0032-recover-expired-message-delta-cursors.md)). The confirmed write does not read the previous value. A stored counter that each writer adjusts would drift the first time one of them gets the transition wrong.
- Graph stores the `$select` projection inside each delta link. `message_delta_reset_link` accepts a `410 Gone` reset only when its `$select` equals `MESSAGE_SELECT` (`crates/providers/microsoft-graph/src/transport/url.rs`). After `MESSAGE_SELECT` changes, a reset for an old cursor is rejected, so stored cursors must be reset.
- Graph v1.0 `mailFolder` has no well-known name. Each alias, such as `inbox` or `archive`, resolves with its own request. EWS `SyncFolderHierarchy` results carry no distinguished id, and the app has no `GetFolder` caller yet.
- `collect_folders` in `crates/providers/exchange-ews/src/provider/mail.rs` starts every EWS hierarchy sync with no sync state. Each listing is therefore a full enumeration, and the `UnreadCount` on each folder is fresh. `map_folder` drops it today.
- `WellKnownMailFolder { Archive, DeletedItems }` in `crates/application/src/mail/provider.rs` serves only the Archive and Delete moves ([ADR 0040](0040-move-messages-to-the-default-archive-folder.md)). EWS moves are not supported ([ADR 0058](0058-per-operation-ews-mail-capabilities.md)).
- No provider string is length-bounded or stripped of control characters today, apart from the response size caps. A preview is body text from an untrusted sender, shown in a list row.

[The mail metadata design](../design/mail-metadata.md) holds the full design, its candidates, and the implementation units M1 to M6.

## Decision

### 1. Store a sanitized message preview

The domain owns `MessagePreview`, a plain-text opening of a message body. Its constructor, `MessagePreview::from_untrusted`, applies one rule to each character of the provider text:

- Cc characters, U+2028, U+2029, and Unicode whitespace become a single space.
- Bidi controls are dropped: U+061C, U+200E–U+200F, U+202A–U+202E, and U+2066–U+2069. They could reorder the visible text of a list row.
- Invisible padding is dropped: U+00AD, U+034F, U+200B, U+200C, U+2060, and U+FEFF. Marketing mail pads its previews with these characters.
- Runs of spaces collapse to one, and the result is trimmed. The function stops after 256 output characters, so its work is bounded.

The constructor returns `None` when nothing printable remains. The rule is idempotent: running it on a stored preview returns the same preview. It runs at both provider boundaries and again when the cache is read, so saved data is revalidated by the same rule. It does no HTML decoding, because both providers send plain text. This rule classifies characters and does not parse markup, so `docs/project/dependency-review.md` records that no crate fits.

Graph requests `bodyPreview` in `MESSAGE_SELECT`. EWS requests `item:Preview` in `message_metadata_shape()`. The transport already requests `Exchange2013_SP1`, the schema that defines `item:Preview`, so the preview needs no server version gate.

`MessageMetadata` gains an optional preview, and `MessageMetadataPatch` gains `preview: FieldUpdate<Option<MessagePreview>>`. A preview never gates `into_complete_message`. A Graph field that is missing changes nothing, an explicit null clears the preview, and a value sets it. EWS `Create` and `Update` changes carry the full shape, so they always set the preview. A read-flag change leaves it untouched.

A preview is sensitive message content. It lives only in the encrypted cache. It is never logged, and its `Debug` output is redacted under [ADR 0048](0048-redact-sensitive-debug-output.md). Tests use synthetic previews only.

### 2. Derive the unread count and never adjust it

`mail_folders` stores two numbers:

- `unread_reported` is the latest provider count. `replace_folders` writes it on every sync.
- `unread_uncached` is `MAX(0, unread_reported - cached_unread)`. It is the number of unread items that the cache cannot see, such as mail older than 90 days.

`unread_uncached` is written only in the `MessageDeltaProgress::Complete` branch of `update_sync_progress`. It runs after `reconcile_full_sync`, in the same transaction, so each accepted complete delta round re-baselines its folder. A stale page is rejected under [ADR 0037](0037-reject-stale-mail-sync-pages.md), so it never re-baselines.

`read_folders` returns `unread_uncached` plus a live count of the folder's cached unread rows. A `NULL` `unread_uncached` returns no count, and the desktop shows no badge. A partial index on unread rows keeps the count cheap.

No writer adjusts a count. Every `is_read` writer changes rows, and the count follows the rows:

- A confirmed read or unread write changes the row. Repeating it changes nothing, so nothing is counted twice.
- A provider-confirmed write whose cache update failed changes no row and no count. The next completed round re-baselines.
- A move removes the row from the source and adds it to the destination.
- A delta upsert, a delta removal, and a cutoff exclusion change the row.
- A replacement scan keeps unseen rows until its final page. The reconcile then deletes them, and the re-baseline runs after that.

Mail that arrives between the folder listing and the folder's completed round is under-reported until the next sync. The clamp at 0 keeps the count valid. Read-state changes made by other clients to mail outside the cache window also wait for the next completed round.

The Graph wire count is `unreadItemCount` in `FOLDER_SELECT`. EWS reads `UnreadCount` from the `Folder` variant of the existing `SyncFolderHierarchy` listing, and other folder variants have no count. A malformed or out-of-range count means no count and never fails a sync. Neither provider sends an extra request for counts.

### 3. Replace `WellKnownMailFolder` with domain `MailFolderKind`

The domain owns `MailFolderKind`, with the variants `Inbox`, `SentItems`, `Drafts`, `Archive`, `DeletedItems`, and `JunkEmail`. It replaces `WellKnownMailFolder` in one change: the move workflow, the `well_known_folder` port, the `MailWorkflowError` variants, the account router, and the desktop. No second list of kinds exists. The variant order carries no presentation meaning.

`MailFolderKinds` holds one resolution: at most one folder for each kind, and at most one kind for each folder. Both providers build it, so the one-to-one rule has one owner.

The `well_known_folder` port keeps serving moves as it does today. Graph adds the four new aliases to `well_known_folder_path` in an exhaustive `match`. EWS keeps returning `UnsupportedOperation`, because EWS moves are not supported (ADR 0058).

### 4. Persist kinds, with `NULL` as unresolved

`mail_folders.kind` holds one of `inbox`, `sent_items`, `drafts`, `archive`, `deleted_items`, `junk_email`, or `other`. `NULL` means the kind is not resolved yet. `other` means the folder is resolved as a plain folder. A unique partial index allows one folder per kind for each account and excludes `NULL` and `other`.

`MailProvider` gains `folder_kinds`, which returns a `MailFolderKinds` for the account. Its default implementation returns `UnsupportedOperation`. The sync workflow calls it after `replace_folders`, and only when some folder in the account has a `NULL` kind. That happens on the first sync, after a new folder appears, and after migration 15. An app restart therefore costs no extra request. The resolution step writes the kind of every cached folder in the account in one transaction. A folder that the resolution names gets its kind, and every other folder gets `other`. `replace_folders` inserts new folders with a `NULL` kind and never overwrites a stored kind. Only the resolution step writes kinds.

### 5. Kind resolution never fails a sync

Kinds are cosmetic. When `folder_kinds` returns any error, the workflow leaves the kinds unresolved, continues with the delta rounds, and retries the resolution on the next sync. A kind that the provider reports as absent is part of a successful resolution, not an error.

### 6. Resolve Graph kinds through aliases

Graph `folder_kinds` sends `GET /me/mailFolders/{alias}?$select=id` for each of `inbox`, `sentitems`, `drafts`, `archive`, `deleteditems`, and `junkemail`. A `404` means that the mailbox has no folder of that kind. Any other failure fails the resolution. A resolved id that is not in the visible folder listing, such as a hidden folder, gets no kind. A JSON `$batch` request could reduce the six requests to one, but it needs a new response parser, so it is deferred.

### 7. Resolve EWS kinds with one batched `GetFolder`

EWS `folder_kinds` follows the behavior of Thunderbird's EWS client (`repos/thunderbird/rust/protocol_shared/src/lib.rs` and `repos/thunderbird/rust/ews_xpcom/src/client/sync_folder_hierarchy.rs`). Thunderbird's client is MPL-licensed, so Einfach Mail copies its behavior and none of its code.

- Send one `GetFolder` request with `BaseShape::IdOnly` over the distinguished ids `inbox`, `deleteditems`, `drafts`, `sentitems`, `junkemail`, and `archive`.
- `archive` is undocumented. Thunderbird found that it returns the account's archive folder and that the documented `archive*` ids return folder-not-found.
- Require the response count to equal the request count, and map each response to its id by request order.
- Treat an id whose response is `ErrorFolderNotFound` or `ErrorSchemaValidation` as absent. Its folder shows as a plain folder.
- Treat any other error response, a count mismatch, a SOAP fault, or a transport failure as a failed resolution. Section 5 applies.

Thunderbird also requests the root folder, and its client fails the folder sync on other errors. Einfach Mail needs no root folder, and its kinds are cosmetic, so it differs from Thunderbird in those two points.

### 8. Migration 15 resets message checkpoints

Append migration 15. Do not edit an earlier migration. It runs this SQL:

```sql
ALTER TABLE mail_messages ADD COLUMN preview TEXT
	CHECK (preview IS NULL OR length(preview) BETWEEN 1 AND 256);
ALTER TABLE mail_folders ADD COLUMN kind TEXT
	CHECK (kind IS NULL OR kind IN (
		'inbox', 'sent_items', 'drafts', 'archive', 'deleted_items', 'junk_email', 'other'
	));
ALTER TABLE mail_folders ADD COLUMN unread_reported INTEGER
	CHECK (unread_reported IS NULL OR unread_reported >= 0);
ALTER TABLE mail_folders ADD COLUMN unread_uncached INTEGER
	CHECK (unread_uncached IS NULL OR unread_uncached >= 0);
CREATE UNIQUE INDEX mail_folders_one_per_kind ON mail_folders (account_id, kind)
	WHERE kind IS NOT NULL AND kind <> 'other';
CREATE INDEX mail_messages_unread ON mail_messages (account_id, folder_id)
	WHERE is_read = 0;
UPDATE mail_account_sync_scope SET query_version = 3;
UPDATE mail_sync_state
SET delta_link = NULL,
	next_link = NULL,
	last_sync_seconds = NULL,
	last_sync_nanoseconds = NULL,
	full_sync_generation = 0,
	full_sync_active = 0,
	checkpoint_revision = checkpoint_revision + 1;
UPDATE mail_messages SET full_sync_generation = NULL;
```

`MESSAGE_QUERY_VERSION` becomes 3. The checkpoint reset follows the flag migration's precedent (`FLAG_SCHEMA_TRANSITION_SQL`). Graph needs it, because old cursors do not return `bodyPreview` and their resets are rejected. EWS needs it, because old items would otherwise never get a preview. The revision increase makes any page in flight from an old checkpoint stale under ADR 0037. The compatibility repairs for versions 5 and 7 run first and need no new repair.

One `MESSAGE_COLUMNS` definition builds every message SQL statement: store, read metadata, read message and `RawMessage`, and the move insert. A test stores, lists, reads, and moves a message with every metadata field set, so a column missing from one statement fails a test.

### 9. Desktop shows the preview, the count, and the kind

- Each message row gains a third line with the preview, in small muted text, truncated to one line, with the `message-preview-{id}` debug selector. `MESSAGE_ROW_HEIGHT` goes from 4.5rem to 5.5rem. The line renders empty when there is no preview, so every row keeps the same height. The preview is appended to the row's accessible details, and the accessible name stays the sender and the subject.
- `NavItem` gains a trailing unread count. It shows only a count above 0 and caps the text at "999+". The accessible name carries the exact number, as in "Inbox, 12 unread". The `folder:{id}` ids and the focus order do not change.
- One exhaustive `folder_icon(Option<MailFolderKind>)` maps each kind to a Lucide icon in the `app_icons!` table: Inbox to `inbox`, Sent Items to `send`, Drafts to `file-pen`, Archive to `archive`, Deleted Items to `trash`, Junk Email to `shield-alert`, and no kind to `folder`.
- `PINNED_FOLDER_ORDER` lists the six kinds in Wino's order: Inbox, Sent Items, Drafts, Archive, Deleted Items, Junk Email. `navigation_order` stable-sorts the pinned folders first and keeps cache order for the rest. A test requires each kind exactly once.
- After a read-state write or a move, the view reloads the cached folders, so the counts follow the rows.

### Amendment of ADR 0011

This ADR amends two bullets of ADR 0011:

- The message-list metadata bullet gains an optional preview. Its rule "Do not request `bodyPreview`" is replaced: request Graph `bodyPreview` and EWS `item:Preview`, and store each as a sanitized `MessagePreview` of at most 256 characters in the encrypted cache. The preview is never logged. Message bodies and attachments stay out of folder listing and delta responses.
- The folder metadata bullet gains a folder kind and a provider-reported unread count.

The rest of ADR 0011 stands.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Derived count, persisted kinds, sanitized preview (chosen) | The count cannot drift from the cached rows, and no `is_read` writer needs new code. Kinds cost no request after they resolve. Each validation lives at one boundary. | The count is briefly low after mail arrives during a sync. A folder that changes kind after resolution, such as an existing folder chosen as the new Archive, keeps its old kind. Upgrading rescans every folder once. |
| Stored counter adjusted by each writer | No count query on read. | Needs transition logic in every `is_read` writer. One missed path drifts silently, and a confirmed write whose cache update failed needs compensation. |
| Workflow adjusts the count, as the issue text suggests | Keeps persistence unchanged. | Needs the previous read state, ignores moves and deltas, and puts cache policy into application code. |
| Count only cached rows | No provider work. | Undercounts any folder with unread mail older than 90 days. |
| Use only the provider count | Simple. | Ignores the user's own reads until the next sync. |
| Baseline and anchor pair (candidate B) | Same result as the chosen count. | One more column for the same value. |
| Kinds cached per provider session (candidate A) | No schema column for kinds. | Resolves again after every app restart, and an Archive folder created during a session appears only after a restart. |
| Two EWS `GetFolder` requests, with `archive` alone | A server that rejects the whole request for `archive` still resolves the other kinds. | Differs from Thunderbird's proven behavior and costs a second request. |
| Kinds from display names or Graph `wellKnownName` | No extra request. | Display names are localized. `wellKnownName` is not on v1.0 `mailFolder`. |
| Keep `WellKnownMailFolder` beside `MailFolderKind` | The move type cannot name Sent Items or Drafts. | Two kind lists that must stay in sync. |
| Sanitize the preview at display time | Raw provider text stays in the cache. | Every reader of the preview needs the rule. |
| Separate migrations for each feature | Smaller schema changes. | Three schema histories for one design, and parallel branches would collide on the migration index. |

## Consequences

- After the upgrade, every folder rescans its message metadata once. Cached opened bodies are discarded once by the delta upserts under [ADR 0034](0034-invalidate-cached-message-bodies-after-delta-updates.md), and they reload when the user opens the message. Counts stay hidden until each folder completes its rescan.
- The cache holds up to 256 characters of body text for every message in the 90-day window, where before it held only opened bodies. The text is encrypted at rest, redacted in `Debug` output, and never logged.
- Graph sends 6 extra requests and EWS sends 1, but only when a folder kind is unresolved. Previews and counts ride on requests that already happen.
- If an Exchange server rejects the whole batched `GetFolder`, for example because it does not accept `archive`, kinds stay unresolved and the request repeats on each sync. A recorded, sanitized response from a real Exchange Server should confirm the per-id behavior before anyone claims it.
- A folder that the mailbox reassigns to a kind after resolution keeps its stored kind until a new folder triggers a resolution. A later change can add a periodic resolution if that matters.
- Message rows are 22% taller.
- `MailFolder::unread_count` carries the reported count when a provider builds the folder and the displayed count when the cache returns it. The port rustdoc states this.
- `docs/project/dependency-review.md` gains an entry that explains why no crate fits the preview rule.
- Verify with the migration tests (a version 14 cache migrates with its rows intact and its checkpoints reset, the constraints reject a 257-character preview, an unknown kind, a negative count, and two inboxes, and the legacy SQLCipher 4.14 fixture still opens), the metadata round-trip test, the count contract tests, synthetic Graph and EWS wire fixtures (including a missing `archive` response), request-count assertions, headless desktop tests for the label, order, height, and folder refresh, and preview screenshots. Do not claim live provider behavior until a test tenant or a recorded, sanitized fixture shows it.

## Sources

- [Mail metadata design](../design/mail-metadata.md) and its [synthesis](../design/mail-metadata/synthesis.md)
- [ADR 0011: Define the initial read-only mail synchronization scope](0011-initial-mail-sync-scope.md)
- [ADR 0032: Recover expired message delta cursors](0032-recover-expired-message-delta-cursors.md)
- [ADR 0034: Invalidate cached message bodies after delta updates](0034-invalidate-cached-message-bodies-after-delta-updates.md)
- [ADR 0037: Reject stale mail sync pages](0037-reject-stale-mail-sync-pages.md)
- [ADR 0058: Gate EWS mail actions by operation](0058-per-operation-ews-mail-capabilities.md)
- [ADR 0066: Use a Fluent visual language and a destination switcher](0066-fluent-visual-language-and-navigation.md)
- [Microsoft Graph message resource](https://learn.microsoft.com/en-us/graph/api/resources/message?view=graph-rest-1.0)
- [Microsoft Graph mailFolder resource](https://learn.microsoft.com/en-us/graph/api/resources/mailfolder?view=graph-rest-1.0)
- [EWS DistinguishedFolderId](https://learn.microsoft.com/en-us/exchange/client-developer/web-service-reference/distinguishedfolderid)
- Thunderbird's EWS client in `repos/thunderbird/rust/`, read for behavior only
