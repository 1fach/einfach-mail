# Mail previews, folder unread counts, and folder kinds

Status: accepted design for [#458](https://github.com/1fach/einfach-mail/issues/458), [#459](https://github.com/1fach/einfach-mail/issues/459), and [#460](https://github.com/1fach/einfach-mail/issues/460). Unit M1 records it, unit M2 adds its domain types, and unit M3 makes the cache store and return them. Units M4 to M6 have not started, so the providers send no preview, count, or kind yet.

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
