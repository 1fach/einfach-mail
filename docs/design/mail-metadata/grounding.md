# Grounding: mail preview, folder unread counts, and folder kinds

## Goal

The owner wants mail rows and folders to look like Wino Mail. Each row should show a preview line, folders should carry unread counts, and Inbox, Sent, Drafts, Junk, Deleted, and Archive should show their own icons in a pinned order. Issues #458, #459, and #460 track this work. One ADR should cover all three because they touch the same layers: provider wire, domain, application, the SQLCipher schema, and desktop UI.

## Constraints

- AGENTS.md treats provider data as untrusted. Parse and bound it at the provider boundary. Domain types must not depend on Graph JSON or database rows. Synthetic fixtures only. Never put real mailbox data in tests, logs, or docs.
- Use Graph v1.0 for Exchange Online. Do not add EWS calls for Exchange Online. EWS is the Exchange Server provider.
- ADR 0011 caches only messages from the last 90 days. ADR 0032 and 0037 cover delta reset and stale-page rejection. ADR 0058 makes EWS mail capabilities per operation.
- Existing caches must migrate. Append a migration and never edit old ones. The SQLCipher 4.14 legacy fixture must still open.
- Every keyboard, accessibility, and test-id contract stays: ADR 0024 message ListBox, `nav-*` ids, and `folder:{id}` ids.

## Provider wire facts

| Field | Graph | EWS |
|---|---|---|
| Preview | `bodyPreview` on `message`. Add it to `MESSAGE_SELECT` (`crates/providers/microsoft-graph/src/transport/url.rs:13`). Plain text, about 255 characters. The delta reset-link check compares `$select` to that constant exactly (url.rs:241-270), so stored delta links taken with the old select must be reset. | `item:Preview` exists from Exchange 2013. The `ews` 0.1.2 crate has `Item.preview: Option<String>`. It is not yet confirmed that the crate parses it from a SyncFolderItems response. Request it in `message_metadata_shape()` (`exchange-ews/src/provider/mail.rs:417`). |
| Unread count | `unreadItemCount` on `mailFolder` v1.0. Add it to `FOLDER_SELECT` (url.rs:12). The folder list is fetched fully on every sync run (`application/src/mail/workflow/sync.rs:45`). | `UnreadCount` is on the `Folder` variant of SyncFolderHierarchy results, but `map_folder` drops it (mail.rs:356). The hierarchy sync reports only structural changes, so counts go stale. Fresh counts need `GetFolder` or a full `FindFolder`. |
| Folder kind | `wellKnownName` is not on `mailFolder` v1.0 (only on `mailboxFolder`). Resolve each kind with `GET /me/mailFolders/{alias}?$select=id`, one request per alias: inbox, sentitems, drafts, junkemail, deleteditems, archive. `well_known_folder_path` already does this for archive and deleteditems. | SyncFolderHierarchy results carry no distinguished id. Resolve kinds with `GetFolder` on distinguished ids and match the returned `FolderId`s. EWS has no `GetFolder` caller yet, and `well_known_folder` returns `UnsupportedOperation` today. |

No provider string (subject, folder name) is length-bounded or control-stripped today, apart from the 8 MiB response caps. A preview newtype would be the first bounded text.

## Domain, application, and persistence facts

- `MailFolder { reference, parent, display_name }` (`crates/domain/src/mail.rs:207`). Debug output redacts the name. `WellKnownMailFolder { Archive, DeletedItems }` lives in application (`application/src/mail/provider.rs:15`) and is used only for moves.
- `MessageMetadataPatch` uses `FieldUpdate<T>` (Unchanged or Set). `into_complete_message` requires subject, from, sender, received_at, and read_state, so a preview must be optional. Graph `WireField` has three states (Missing, Null, Value). A missing preview must not clear the stored one.
- Sync order is `list_folders`, then `replace_folders` (upsert of `mail_folders` with a temporary seen table), then a delta loop per folder through `commit_delta_page`, which runs `apply_message_patch` and `store_message_metadata`. Any delta upsert clears the cached body.
- Every SQL column list is hard-coded: `store_message_metadata`, `read_cached_metadata`, `read_cached_message` and `RawMessage`, `writes.rs` move inserts, and `folders.rs` insert and read. `read_folders` orders by display_name.
- Migrations are an append-only list of 14 entries (`persistence/src/shared/schema/migrations.rs`). The flag migration added a column and reset `mail_sync_state` (delta_link and next_link set to NULL, `query_version = 2`) so old rows refetch. `compatibility.rs` special-cases versions 5 and 7. Per-migration tests live in `schema/tests.rs`.
- The read-state write path is `MailWorkflow::set_message_read_state` (`workflow/actions.rs:50`), then the provider, then `cache.set_message_read_state` (`persistence/mail/store.rs:390`): an UPDATE of `is_read` plus clearing a marker, in one transaction. The previous `is_read` is not read, so a naive count adjustment would double count. Moves and delta removals also change counts.
- Unread counts: the cache holds only 90 days (ADR 0011), so counting rows locally undercounts. The provider count is correct but refreshes only at `list_folders`. The candidate shape is the provider count as baseline plus local adjustments on confirmed read-state changes, with a transition-aware update, reset at each folder refresh.

## Desktop UI facts

- Rows: `render_message_row_content` (`apps/desktop/src/features/mail/view/message_list.rs:315`) has two lines. `MESSAGE_ROW_HEIGHT = rems(4.5)` (`view.rs:52`) drives the row height and the virtual list sizes (`day_groups.rs:56`). `MessageRowSummary` builds the display strings and the accessible label and details.
- Folders: `folder_navigation.rs` creates `NavItem::new(id, name, AppIcon::Folder)` for each folder in cached order. `NavItem` has no trailing count slot. The folder pane is not refreshed after a read-state write.
- Icons: the `app_icons!` table in `components/icon.rs` generates `AppIcon`, the embedded Lucide subset, and a test list.
