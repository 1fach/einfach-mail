# Mail previews, folder unread counts, and folder kinds

Status: accepted design for [#458](https://github.com/1fach/einfach-mail/issues/458), [#459](https://github.com/1fach/einfach-mail/issues/459), and [#460](https://github.com/1fach/einfach-mail/issues/460). Unit M1 records it, unit M2 adds its domain types, unit M3 makes the cache store and return them, and units M4 (PR #486) and M5 (PR #485) make Graph and EWS send all three. Units M1 to M6 are done. The desktop (unit M6, PR #487) shows the preview line, the folder counts, and the kind icons in the pinned order.

The Fluent restyle ([the Fluent workspace design](fluent-workspace.md)) used only data the app already had. This design adds the three pieces of mail data that Wino Mail shows and Einfach Mail does not: a preview line under each message, an unread count on each folder, and a kind for Inbox, Sent Items, Drafts, Archive, Deleted Items, and Junk Email, which gives those folders their own icons and a pinned order. Wino Mail (`repos/wino-mail/`) is GPL-3.0 and is a visual reference only. Thunderbird's EWS client is MPL-licensed. The design copies its behavior, never its code.

## Read in this order

1. [Synthesis](mail-metadata/synthesis.md) is the accepted design. It names the base candidate, the owner decisions, the grafts, the parts kept from the base, and the implementation units M1 to M6.
2. [Candidate A](mail-metadata/candidate-a.md) is the base design. It holds the domain types, the preview rule, the unread count derivation, the provider work, the migration, and the UI. The synthesis overrides it where they differ.
3. [Grounding](mail-metadata/grounding.md) maps the current code and the provider wire facts that both candidates worked from. Candidate A corrects one of its claims: EWS folder listings are full enumerations, so their unread counts do not go stale.
4. [Candidate B](mail-metadata/candidate-b.md) is the alternative design. Read it for the grafted ideas and the rejected alternatives.

[ADR 0067](../decisions/0067-message-previews-folder-unread-counts-and-kinds.md) records the lasting decision. It amends the message and folder metadata bullets of [ADR 0011](../decisions/0011-initial-mail-sync-scope.md), including its "Do not request `bodyPreview`" rule.

## Decisions from the repository owner

These were made on 2026-11-14. Record any later change here and in ADR 0067.

- Store message previews. A preview is sanitized, holds at most 256 characters, lives in the encrypted cache, and is never logged or shown in `Debug` output.
- Accept a one-time resync after the upgrade. Migration 15 resets every message checkpoint, so each folder rescans once. Cached opened bodies are discarded once and reload when the user opens the message.
- Resolve the EWS Archive folder the way Thunderbird's EWS client does: one batched `GetFolder` over the distinguished ids, including the undocumented `archive` id. An id that answers `ErrorFolderNotFound` or `ErrorSchemaValidation` is absent, and its folder shows as a normal folder.

## Defaults chosen during design

The owner can change these.

- Pinned order follows Wino: Inbox, Sent Items, Drafts, Archive, Deleted Items, Junk Email, then the other folders in cache order.
- Every folder shows its unread count, as Wino does. Outlook hides it for Sent Items and Deleted Items.
- The badge caps at "999+". The accessible name carries the exact number, as in "Inbox, 12 unread".
- Message rows are 5.5rem tall whether or not a preview exists, so the virtual list keeps uniform sizes.

## Unit map

| Unit | Work | Issues | Depends on |
|---|---|---|---|
| M1 | ADR 0067, the ADR 0011 amendment, and these design docs | #458, #459, #460 | none |
| M2 | Rename `WellKnownMailFolder` to domain `MailFolderKind` and add the domain types. No behavior change. | #458, #459, #460 | M1 |
| M3 | Persistence: migration 15, `MESSAGE_COLUMNS`, the preview, kind, and unread columns, the workflow kind resolution step, and the `MailProvider::folder_kinds` port | #458, #459, #460 | M2 |
| M4 | Graph: `bodyPreview`, `unreadItemCount`, and `folder_kinds` through six alias lookups | #458, #459, #460 | M3 |
| M5 | EWS: `UnreadCount`, `item:Preview`, and `folder_kinds` through one batched `GetFolder` | #458, #459, #460 | M3, in parallel with M4 |
| M6 | Desktop: the third row line, the folder unread count, kind icons, pinned order, and a folder refresh after writes | #458, #459, #460 | M3. Merges last. |

The synthesis lists the full scope of each unit. M3 must merge before M4, because the Graph select change must not reach users before the checkpoint reset. Land migration 15 alone, so that no other branch claims index 15. M4 and M5 need synthetic wire fixtures before anyone claims live provider behavior.

## Progress log

Each unit appends one line when it merges: date, unit, PR, and verification evidence.

- 2026-11-14, M2 ([PR #481](https://github.com/1fach/einfach-mail/pull/481)): `MailFolderKind` replaces `WellKnownMailFolder` in every caller, and the domain gains `MessagePreview`, `UnreadCount`, `MailFolderKinds`, the `MailFolder` and `MessageMetadata` builders, and `set_preview`. Providers pass no preview, kind, or count. `bash scripts/check-linux-local.sh` passed. The workspace test count went from 658 to 693 passed, with 3 ignored both times. Graph resolves all six aliases through one exhaustive match, and EWS still answers `UnsupportedOperation`.
- 2026-11-14, M3 ([PR #482](https://github.com/1fach/einfach-mail/pull/482)): migration 15 adds the preview, kind, and unread columns and resets every message checkpoint once. `MESSAGE_COLUMNS` builds every message statement, an accepted `Complete` page re-baselines the folder count after the reconcile, and the sync resolves kinds only while a folder is unresolved. Providers still send no preview, count, or kind. `bash scripts/check-linux-local.sh` passed. The workspace test count went from 693 to 722 passed, with 3 ignored both times, and the desktop `ui-preview` run passed 224.
- 2026-11-14, M4 ([PR #486](https://github.com/1fach/einfach-mail/pull/486)): Graph requests `bodyPreview` and `unreadItemCount` and implements `folder_kinds` through six `$select=id` alias lookups, where a `404` means an absent kind. A reset link with the old message select is rejected. `bash scripts/check-linux-local.sh` passed all 13 steps. The workspace test count went from 722 to 730 passed, with 3 ignored both times, the Graph crate went from 122 to 130, and the desktop `ui-preview` run passed 224. Synthetic `httpmock` fixtures cover every Graph path. No live Graph request was made.
- 2026-11-14, M5 ([PR #485](https://github.com/1fach/einfach-mail/pull/485)): EWS reads `UnreadCount` from the `Folder` variant of the hierarchy listing, requests `item:Preview` and sets it on `Create` and `Update` changes, and resolves folder kinds with one `GetFolder` over six distinguished ids. Synthetic SOAP fixtures cover the hierarchy counts, the previews, a fully resolved `GetFolder`, a missing `archive` with `ErrorFolderNotFound` and with `ErrorSchemaValidation`, other error codes, and response-count mismatches. `bash scripts/check-linux-local.sh` passed. The workspace test count went from 722 to 732 passed, with 3 ignored both times, and the desktop `ui-preview` run passed 224. No live Exchange server was available, so no real `GetFolder` response has been seen.
- 2026-11-14, M6 ([PR #487](https://github.com/1fach/einfach-mail/pull/487)): message rows are 5.5rem tall with a reserved third line for the preview, folders show their unread count and kind icon, the six kinds come first in Wino's order, and a read-state write or a move reloads the cached folders so the counts follow the rows. Headless desktop tests cover the row height, the preview line, the accessible details, the count text and label, the icon map, the pinned order, the tab order, and the refresh after a read-state write and after a move. Screenshots of Mail and the reader, light and dark, at 1280x800 and 760x700 are under `docs/project/evidence/mail-metadata-*.png`. All of it uses synthetic `example.test` data, so no real Graph or Exchange mailbox has shown a preview, a count, or a kind.

## Decisions while the owner was away

These were taken without the owner. The owner can reverse any of them.

From the review of [PR #480](https://github.com/1fach/einfach-mail/pull/480):

1. Moves keep their current alias lookup and do not read persisted kinds, so no behavior changes.
2. `MailFolderKinds::covers` is dropped. M2 does not add it.
3. A folder that later becomes Archive or Junk keeps its stored kind. This matches Thunderbird, which builds its well-known map only on a full hierarchy sync (`repos/thunderbird/rust/ews_xpcom/src/client/sync_folder_hierarchy.rs:43`).
4. A whole-batch EWS rejection retries on each sync. This is Thunderbird's behavior, it is cosmetic, and it costs one request. It stays until someone records a sanitized response.

Decisions in M2:

5. The desktop uses `MailFolderKind` directly and does not add a two-variant enum for the offered shortcuts. ADR 0067 section 3 says no second list of kinds exists. The move UI still offers only Archive and Deleted Items. Where a desktop `match` must name the four other kinds, it uses the generic move behavior: the `Move` capability, the generic move ids and copy, and "Open folder". Those arms run only if a later change offers another kind.
6. `MailFolderKinds` stores its pairs in a `Vec` and not a `HashMap`. It holds at most six pairs, and one list of pairs cannot disagree with itself the way two maps could.
7. `MessagePreview` stops before a character that would not fit, so a cut never leaves a trailing space and the rule stays idempotent. Wino Mail stores Graph's `bodyPreview` as sent (`repos/wino-mail/src/Wino.Core/Extensions/OutlookIntegratorExtensions.cs:66`), with no limit or cleaning. That is the behavior the ADR's rule replaces for safety, so the 256-character rule follows the ADR alone.
8. U+200D, the zero-width joiner, is kept. It joins emoji sequences, and the ADR's drop list does not include it. `dependency-review.md` records that `zero-width-strip` drops it.
9. `MailFolder` and `MessageMetadata` `Debug` output shows the kind, the unread count, and `MessagePreview(<redacted>)`. Counts and kinds are not message content.

Decisions in M3:

10. A cache failure in the kind resolution step fails the sync. Only a failure of `MailProvider::folder_kinds` is ignored, which is what ADR 0067 section 5 covers. The two cache calls fail only when the database is unavailable, and then the next statement of the sync fails too. Wino Mail fails its whole sync when a special-folder lookup throws (`repos/wino-mail/src/Wino.Core/Synchronizers/OutlookSynchronizer.cs:3165`), so Einfach Mail is already more lenient than Wino for provider errors.
11. `MESSAGE_COLUMNS` holds the ten metadata columns. The key columns, the body columns, and `full_sync_generation` stay in each statement, because the statements treat them differently. Each column records whether it can be NULL, and a move fills only the nullable columns of an existing destination row, as it did before. Rows are read by column name and the store statement binds by name, so the order of the list has no meaning.
12. There is no folder column list. The folder write stores `unread_reported` and never `kind` or `unread_uncached`, and the folder read returns `kind` and the derived count, so the two statements share no column set. They share one SQL fragment for the cached unread count instead, because the count drifts if the read and the re-baseline count different rows.
13. `MailFolderKinds` gains `assignments()`. The cache needs each pair to store a resolution. Without it, persistence would keep its own list of the six kinds, and ADR 0067 section 3 allows no second list.
14. `store_folder_kinds` marks every cached folder as `other` and then sets the named folders, in one transaction. A later resolution can therefore move a kind to another folder without breaking the one-folder-per-kind index, and a folder that the provider no longer names loses its kind. This does not change decision 3: a resolution still runs only while a folder is unresolved.
15. `replace_folders` ignores the kind on every listed folder, including a new one. A provider count of `None` clears `unread_reported`, so the folder shows no count after its next completed round.
16. The account router forwards `folder_kinds` now. M4 and M5 run in parallel, and neither has to change the router.
17. The desktop test cache reports no unresolved kind and ignores a stored resolution. The desktop preview and test providers keep the default `UnsupportedOperation`.
18. The test for a delta removal commits the removal on a page that does not complete the round. A final page re-baselines to the provider's count in the same transaction, so it would hide the effect of the removed row. In a real sync the folder listing brings the lower provider count before that final page.
19. Migration 15 increments `checkpoint_revision`, so `replacement_generation_migration_preserves_an_existing_snapshot` now expects revision 1 after it migrates a version 1 cache to the latest schema. The tests for the version 5 and version 7 repairs also check that the new columns exist.
20. The round-trip test reads the `mail_messages` columns from the schema, not from `MESSAGE_COLUMNS`, and expects only `full_sync_generation` to be NULL. A column that a later migration adds and the store statement leaves out then fails the test, even when `MESSAGE_COLUMNS` also lacks it.

Decisions in M5:

21. `GetFolder` asks for `inbox`, `deleteditems`, `drafts`, `sentitems`, `junkemail`, and `archive`, in that order. This is Thunderbird's list without `msgfolderroot` and `outbox` (`repos/thunderbird/rust/protocol_shared/src/lib.rs:47`), and ADR 0067 section 7 names the same order. The order only pairs each response with its id.
22. A `Warning` response class counts as a success. Thunderbird's `process_response_message_class` does the same (`repos/thunderbird/rust/ews_xpcom/src/client.rs:439`), and so do the other EWS reads in this crate.
23. A success must hold exactly one `Folder` with a folder id. An empty list, a folder with no id, a search folder, or two folders fail the whole resolution. Thunderbird's `validate_get_folder_response_message` applies the same rule and also fails (`repos/thunderbird/rust/ews_xpcom/src/client.rs:452`).
24. A folder that two ids name keeps the kind of the earlier id, and the later kind is absent. Thunderbird differs: it collects the pairs into a map keyed by folder id, so the later id wins (`repos/thunderbird/rust/ews_xpcom/src/client/sync_folder_hierarchy.rs:388` to `:418`). `MailFolderKinds::assign` keeps the first pair, and the M5 scope said to skip a duplicate. Neither choice has been seen against a real server.
25. Other error codes go through the crate's existing `map_response_code`: `ErrorAccessDenied` and `ErrorImpersonateUserDenied` become `AccessDenied`, and every other code becomes `InvalidResponse`. The sync workflow ignores both.
26. A `Create` or `Update` change without `Preview` clears the stored preview. Thunderbird passes an empty preview in that case (`repos/thunderbird/rust/ews_xpcom/src/client/sync_messages_for_folder.rs:185` and `:232`), and ADR 0067 section 1 says these changes always set the preview.
27. `item:Preview` has no version gate. Thunderbird requests it only from Exchange 2013 or later (`sync_messages_for_folder.rs:125`). Einfach Mail sends `RequestServerVersion` `Exchange2013_SP1` on every request, so a server older than Exchange 2013 would reject the request whether or not it asks for a preview. That is inferred from the EWS schema versions, not tested.
28. A malformed `UnreadCount` fails the whole hierarchy listing, not only the count. The `ews` crate parses the field as `u32` together with the rest of the response, and it did so before M5. A temporary test showed that `-1`, `abc`, and `4294967296` each fail the parse. Making the count lenient needs a parser outside the `ews` crate, and ADR 0058 already chose not to fork the crate for one field. M5 adds no new failure path, and ADR 0067 section 2 now states this limit.
29. Thunderbird gives no basis for counts. Its EWS client never reads `UnreadCount` from a folder and sets it only to `None` when it creates or updates one (`repos/thunderbird/rust/ews_xpcom/src/client/create_folder.rs:45`). The count follows ADR 0067 alone.

Decisions in M4:

30. Graph reads `unreadItemCount` from any JSON value. Only an integer from 0 to `u32::MAX` becomes a count. A negative number, a fraction, a string, null, or a number too large for `i64` becomes no count, and the folder stays in the listing. The unit plan named an `Option<i64>` wire field, but that field rejects the whole folder listing for a fraction or an oversized number, and ADR 0067 section 2 says a malformed count never fails a sync.
31. The move lookup shares `FOLDER_SELECT`, so it now also requests `unreadItemCount` and ignores it. It still parses the same `WireFolder`, and no move behavior changes.
32. Kind resolution has its own `$select=id` path and its own `GraphReadOperation::FolderKind`. The transport turns a `404` for that operation into `GraphResponse::FolderNotFound`, which is a response and not an error. Every other status keeps the standard mapping. The requests send no immutable-ID `Prefer` header, because the resolved IDs must match the IDs of the folder listing, which uses default IDs. Wino Mail adds the immutable-ID header to every Graph request (`repos/wino-mail/src/Wino.Core/Synchronizers/OutlookSynchronizer.cs:1728`), so its listing and alias IDs match the same way.
33. Graph requests the aliases one at a time in the order `inbox`, `sentitems`, `drafts`, `deleteditems`, `junkemail`, `archive`. When two aliases name one folder, the earlier alias keeps it. This is the precedence of Wino's kind mapping (`OutlookSynchronizer.cs:2820`). Archive comes last because the user chooses the Archive folder, so it is the alias most likely to name a folder that already has a kind. Thunderbird's Graph client lets the later alias win, because it builds a map from folder ID to alias (`repos/thunderbird/rust/graph_xpcom/src/client/sync_folder_hierarchy.rs:161`).
34. Graph differs from Wino in two points. Wino fails the sync when any alias lookup fails, including a missing folder (`OutlookSynchronizer.cs:3147`). Einfach Mail treats a `404` as an absent kind and ignores other failures under ADR 0067 section 5. Wino also reuses stored IDs only when all six kinds are stored (`OutlookSynchronizer.cs:3176`), so a mailbox with no Archive folder sends six requests on every sync. Einfach Mail stores the absent kind as `other` and sends the six requests again only for an unresolved folder. Thunderbird's Graph client also fails on a missing well-known folder, and its code marks that case as unknown.
35. An alias that answers with an empty ID fails the resolution with `InvalidRecord`. Wino also fails on an alias with no ID (`OutlookSynchronizer.cs:3160`).
36. The Graph crate cannot depend on persistence, so the request-count test runs the provider calls that each sync makes. The first sync lists folders, resolves kinds, and reads a folder, which sends exactly 6 alias requests. The later sync lists folders and reads a folder, which sends none. The workflow test `folder_kinds_resolve_once_and_again_only_when_a_new_folder_appears` in `crates/persistence/src/workflow_tests/mail/folder_metadata.rs` proves that a sync with every kind stored does not call `folder_kinds`.

Decisions in M6:

37. The unread count is plain trailing text, not a GPUI Kit `Badge`. The design guide allows a badge for a count but asks for neutral badges and a restrained hierarchy (`design-guides.md:152` to `:156`, `:130` to `:133`), and a count on every folder would put a filled chip on most rows beside the selection pill. Wino Mail uses an accent-filled `InfoBadge` (`repos/wino-mail/src/Wino.Mail.WinUI/Styles/ShellMenu/ShellMenuTemplates.xaml:284`), and Einfach Mail differs on purpose. See `apps/desktop/src/components/nav_item.rs:169`.
38. The count on the selected item uses the foreground color, not the muted one. A pixel measurement of the first screenshot gave 4.31:1 for the light theme's muted text on the selection fill, below 4.5:1 for small text. After the change the selected count measures 11.6:1. The unselected count stays muted and measures 5.75:1 in the light theme and 9.74:1 in the dark theme. Same function, `nav_item.rs:169`.
39. Every folder with a count above 0 shows it, including Sent Items and Deleted Items. This keeps the earlier default. Wino hides the badge only at 0 (`repos/wino-mail/src/Wino.Mail.WinUI/Helpers/XamlHelpers.cs:161`).
40. Only a labeled `NavItem` shows a count. An icon-only item ignores it, so its accessible name never promises a number it does not draw (`nav_item.rs:117`).
41. The preview line has a fixed height of 1rem and is always rendered, so the three lines sit at the same offsets in every row (`message_list.rs:323`). Wino collapses the line when the text has no letter (`XamlHelpers.cs:157` and `MailItemDisplayInformationControl.xaml:233` to `:241`). Einfach Mail does not copy that rule. The uniform row height is the accepted design, and a preview such as "2026-0412" is still content.
42. The preview uses `muted_foreground` at full opacity. Wino dims its preview to opacity 0.7 (`MailItemDisplayInformationControl.xaml:238`), and a dimmed token would fall under the contrast floor. A pixel measurement of the first screenshot gave 6.07:1 on the list and 5.05:1 on the selected row.
43. The preview is the last item of the accessible details, after "Flagged" and joined with ". ", and the accessible name is unchanged (`message_list.rs:287`). A screen reader reads the sender and subject first and the preview last.
44. `navigation_order` runs on the copy the folder pane renders, not on `MailView::folders` (`folder_navigation.rs`, `folder_kind.rs:32`). The Move menu keeps cache order, and `preferred_folder` still selects the folder named "Inbox" (`synchronization.rs:270`). Selecting by kind would help a localized Inbox name. That is a separate change.
45. The folder reload runs after every read-state write and every move, including a failed one, through `list_cached_folders().await.ok()` (`read_state.rs:54`, `message_move.rs:152` and `:198`). A failed read keeps the folders on screen. A follow-up-flag write does not change a count, so it does not reload. A sync already reloads the folders through `apply_snapshot`.
46. The desktop test cache derives its counts the way persistence does. It adjusts a folder's count by one when a message changes read state or moves, and a folder with no count keeps none (`test_support/mail/cache.rs:48`). Without it, the refresh tests would only compare two copies of the same static count.
47. `AppIcon::DeletedItems` is its own row that maps to `trash`, though `AppIcon::Delete` already does (`icon.rs:48`). The Delete command and the Deleted Items folder can then change icons separately. `app_icons!` already holds duplicate Lucide names, such as `Mail` for two variants.
48. The pinned-order test checks the array against an exhaustive `match` in the test and not against `EnumSet::all()` (`folder_kind/tests.rs:10`). `MailFolderKind` is not an `EnumSetType`, and deriving it would change the domain for a desktop test. Adding a seventh kind stops the test from compiling until the array, the icon table, and the test name it.
49. The preview scenario lists its folders in a cache order that is not the pinned order, and it finds the Inbox by kind and not by position (`app/preview.rs:488` and `:567`). Its folder IDs `sent-items`, `junk-email`, and `deleted-items` now match the IDs its `well_known_folder` answers with. It had used `sent`. Its counts are 12, 1,204, 4, 3, 7, and 0 for the rest, so the screenshots show a count, the "999+" cap, and a hidden zero.
50. Tab order starts at New mail, then follows the pinned order and not the cache order, because the pane renders in that order and the focus order is the render order. The `folder:{id}` ids are unchanged. A test reverses the cache order and checks the tab sequence (`tests/folder_navigation.rs`).
