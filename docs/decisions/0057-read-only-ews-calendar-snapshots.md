# ADR 0057: Read-only EWS calendar snapshots

- Status: Accepted
- Date: 2026-11-09
- Owners: Einfach Mail maintainers

## Context

Issue [#266](https://github.com/1fach/einfach-mail/issues/266) extends the Linux on-premises Exchange account from read-only mail to read-only primary-calendar views. Accepted [ADR 0053](0053-linux-read-only-exchange-ews.md) leaves EWS calendars unavailable; this decision changes that scope while keeping Exchange Online on Graph and EWS writes unavailable.

The existing Graph calendar workflow uses fixed-range `calendarView/delta` queries and opaque per-view cursors. EWS has different semantics. Microsoft documents `FindItem` with `CalendarView` as the range query that expands recurrence occurrences and exceptions. `SyncFolderItems` does not apply calendar recurrence logic. `CalendarView` exposes a result limit but no indexed offset. Indexed paging uses another view type and does not expand recurrence. The `FindItem` response includes `IncludesLastItemInRange`, which can identify an incomplete result.

The domain view already contains explicit UTC instant bounds and the user's local date bounds. EWS `CalendarView` accepts UTC `StartDate` and `EndDate`, so the request does not need to translate the local IANA zone into a Windows ID. EWS calendar items expose `Start`, `End`, `IsAllDayEvent`, and original `StartTimeZoneId` and `EndTimeZoneId` fields. The EWS response time values include an explicit UTC or numeric offset. Recover all-day civil dates in each event's original start/end zone rather than treating its UTC date as a calendar date.

The workspace already uses Thunderbird's `ews` 0.1.2 crate for EWS mail. It provides `FindItem`, `View::CalendarView`, and the response's range-completeness metadata, but its typed `Message` has no calendar `Start`, `End`, all-day, location, or time-zone fields. Existing `quick-xml` and Serde dependencies can map only these provider-specific fields without adding a general XML implementation. Wino Mail's calendar code uses Microsoft Graph `CalendarView.Delta`; it is not an EWS calendar implementation. Thunderbird's calendar provider is also Graph-based.

## Decision

Add Linux read-only EWS primary-calendar views in M8:

- Set EWS account capabilities to read-only mail and read-only calendar. Expose agenda reads through the native account session and router. Keep event creation, editing, and deletion unavailable in the UI and rejected by the provider boundary.
- Query the primary calendar for the selected fixed range with EWS `FindItem` and `CalendarView`, serializing the existing UTC instant bounds explicitly. Request only the item ID, subject, location, start/end, all-day flag, and original start/end time-zone fields needed by the provider-neutral occurrence model. Rely on Exchange's recurrence expansion; do not fetch recurring masters through `SyncFolderItems` or expand recurrence locally.
- Model Graph delta pages and EWS complete range snapshots as different provider results. Graph keeps its existing opaque cursor. EWS must not use a fabricated delta link. Add an application-owned cache operation that atomically replaces one exact account/view snapshot after the complete response has been validated.
- Require `IncludesLastItemInRange` to be present and true, and enforce the existing transport response-byte limit plus a fixed event-count bound. Return a resource-limit error for an incomplete or over-limit range. Keep the last complete cached snapshot visible and leave the cache unchanged on request, parse, validation, or commit failure.
- Map required EWS values at the provider boundary into `CalendarOccurrence`. Normalize timed values to UTC. For all-day values, resolve each event's Windows time-zone ID to its default IANA/tzdb ID and use the existing Jiff time-zone database to recover local start and exclusive end dates. Preserve the original EWS time-zone IDs as opaque provider values. Use `windows-timezones` 0.5.1 without its optional Chrono integration; compile it on the workspace MSRV and include the Unicode data license notice in third-party attributions. Do not add a hand-maintained mapping table. If an all-day zone is missing, custom, or unknown, fail the entire snapshot so the app does not display an incomplete calendar as complete.
- Require an explicit timezone in each EWS `Start` and `End` wire value. Do not add `TimeZoneContext` for the fixed UTC range query. The selected request version is Exchange 2013 SP1, which supports per-item `StartTimeZone` and `EndTimeZone` fields; this avoids synthesizing an account-wide SOAP time-zone definition.
- Use the existing `ews` operation types and `quick-xml` for narrowly scoped calendar response mapping. Do not vendor or fork the full EWS crate to add its missing calendar fields.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| `FindItem` with `CalendarView` and an atomic full snapshot | Uses Exchange recurrence expansion and matches the application's fixed-range agenda. A completion flag prevents partial results from replacing a good cache. | Dense ranges can exceed the result bound and must report an error; there is no continuation cursor. |
| Use `SyncFolderItems` and expand recurring masters locally | Has an EWS synchronization state and can fetch item changes. | EWS does not apply calendar recurrence logic to this operation. Local expansion would need to reproduce recurrence rules, modified exceptions, deleted occurrences, and time-zone behavior. |
| Use `FindItem` with indexed paging | EWS documents indexed paging for large item sets. | Indexed item views do not expand recurring occurrences. Combining the results with local expansion would add the same recurrence and identity problems. |
| Split dense `CalendarView` ranges into smaller time intervals | Could retrieve more events while retaining server-side expansion. | Overlapping events and occurrences need stable deduplication; a single dense instant can still exceed the bound; partial results require resumable staging. Defer until real Exchange responses show the fixed-range limit is a user problem. |
| Send `TimeZoneContext` for the Linux system zone | Could set a default interpretation zone for EWS DateTime fields. | The fixed view query already sends explicit UTC bounds, and the application needs each event's original zone for all-day dates. A request-wide zone would not replace per-item zones and would require serializing a full EWS timezone definition. Do not add it unless sanitized Exchange responses show it is required. |

## Consequences

- The product brief adds read-only EWS calendar access, while the roadmap keeps it in a separate M8 outcome after the base M7 EWS account path.
- The application cache stores a complete snapshot separately from a Graph delta page. The current delta cursor and reset behavior remain Graph-specific.
- The EWS response mapper is provider code because the selected Rust EWS crate has no typed calendar item fields. It uses `quick-xml`, Serde, the existing `ews` operation types, and the existing transport. `windows-timezones` 0.5.1 maps Windows IDs to CLDR's default tzdb IDs, and Jiff resolves those IDs. The workspace Rust 1.95 compiles the mapper. The crate declares no MSRV. Its Unicode license terms are retained in [`LICENSES/Unicode-DFS-2016.txt`](../../LICENSES/Unicode-DFS-2016.txt).
- The provider exposes a complete EWS view snapshot through a separate provider result. `CalendarProvider` reports whether the active provider uses delta pages or complete snapshots. `CalendarWorkflow` enforces the response-byte budget and passes one completed snapshot to `CalendarCache::replace_snapshot`; the SQLCipher adapter replaces one exact view atomically and stores completion outside the Graph cursor table. Account capabilities and the agenda UI still expose Graph calendars only, so EWS snapshots are not available to users yet.
- `localzone` 0.3.1 offers a similar bidirectional CLDR map under Apache-2.0, but its latest release is from April 2024 and it is not the selected dependency.
- Provider fixtures cover request construction, empty and populated responses, recurrence occurrence and exception mapping, time-zone conversion, completeness checks, and provider limits. Application and SQLCipher tests cover empty snapshots, account and view isolation, rollback, and reopen. No Exchange Server or sanitized calendar response recording is available, so live interoperability remains unverified and must not be claimed.

## Application and cache integration

The EWS provider returns one complete bounded snapshot, and the application workflow now selects synchronization behavior explicitly. Do not call the snapshot method and treat `UnsupportedOperation` as a strategy probe.

- `CalendarProvider` reports an application-owned strategy with distinct `Delta` and `Snapshot` variants. The account router delegates to the selected provider. Graph keeps its existing `sync_round` path and opaque links; EWS requests one complete snapshot.
- Both strategies use the existing per-view reservation. The snapshot workflow applies the application response-byte bound, calls the cache once, and marks the in-memory account recovery state synchronized only after the cache commit succeeds. A snapshot sync report counts its one committed response.
- `CalendarCache::replace_snapshot` validates that each occurrence references the requested view, overlaps it, and has a unique event ID. An empty complete response removes all events in that view.
- The SQLCipher adapter stores EWS occurrences in the existing `calendar_events` table. One transaction replaces rows for the exact account and range, stores the snapshot completion time, and removes stale replacement staging rows. A write error rolls back the events and sync status together.
- Snapshot completion lives in a separate per-view state table. The cache exposes a tagged snapshot state without a `CalendarDeltaLink`; the Graph delta-link constraint remains unchanged. A Graph page commit clears snapshot metadata for that exact view in the same transaction.
- Return read-only mail and read-only calendar access from the EWS account profile. Reuse the existing `CalendarSession`, provider router, and native `CalendarView`; do not add an EWS-specific session or agenda.
- Let `CalendarView` own write-command visibility and entry points. A read-only account keeps week navigation, sync, and event selection. The view shows a `Read-only` state, omits event-write controls, and does not open the event composer from pointer or keyboard actions. Keep the EWS provider's unsupported-operation results as the boundary for direct write calls.
- Test the EWS profile capabilities, account navigation to the shared calendar view, read-only agenda controls and keyboard behavior, and provider write rejection. Keep Graph account behavior unchanged.

### Agenda alternatives

| Design | Benefits | Costs and risks |
| --- | --- | --- |
| Reuse `CalendarView` and gate writes by `CalendarAccess` | Keeps one agenda, sync flow, and keyboard model. The existing account profile already carries the capability. | `CalendarView` must check the capability in both rendering and command entry points. |
| Keep write commands visible but disabled | Shows users which Graph accounts can change events. | Adds controls that cannot act in this account and repeats the reason beside each one. |
| Add an EWS-specific agenda and session | Isolates EWS presentation from Graph. | Duplicates week navigation, event selection, loading, errors, and accessibility behavior. |

Reuse the existing agenda. Show one read-only state, keep calendar viewing actions available, and omit event-write controls for EWS accounts.

This design keeps the old active snapshot readable until a complete EWS response has been validated and its single cache transaction commits. EWS response bodies are already bounded to 8 MiB by the transport and the request is limited to 512 occurrences, so a separate multi-page staging generation is unnecessary. The snapshot workflow will also enforce the existing 64 MiB sync-response budget as a second boundary.

| Alternative | Reason not selected |
| --- | --- |
| Reuse the Graph delta link with a sentinel or fabricated value | Delta links are opaque Graph protocol state and must never represent an EWS snapshot. |
| Remove the delta-link database constraint by rebuilding the existing sync-state table | It changes the established Graph cursor invariant and requires a migration around tables that reference the per-view key. Separate snapshot metadata is narrower and preserves the current Graph representation. |
| Store EWS events in a second active-events table | It duplicates event persistence and requires merging two tables for each agenda read. The event rows are already provider-neutral and share the same account/view key. |

## Sources

- [Microsoft: CalendarView](https://learn.microsoft.com/en-us/exchange/client-developer/web-service-reference/calendarview)
- [Microsoft: FindItem operation](https://learn.microsoft.com/en-us/exchange/client-developer/web-service-reference/finditem-operation)
- [Microsoft: Recurrence patterns and EWS](https://learn.microsoft.com/en-us/exchange/client-developer/exchange-web-services/recurrence-patterns-and-ews)
- [Microsoft: CalendarItem](https://learn.microsoft.com/en-us/exchange/client-developer/web-service-reference/calendaritem)
- [Microsoft: Time zones and EWS](https://learn.microsoft.com/en-us/exchange/client-developer/exchange-web-services/time-zones-and-ews-in-exchange)
- [Microsoft: TimeZoneContext](https://learn.microsoft.com/en-us/exchange/client-developer/web-service-reference/timezonecontext)
- [Thunderbird `ews` crate](https://github.com/thunderbird/ews-rs)
- [`ews` 0.1.2 API documentation](https://docs.rs/ews/0.1.2/ews/)
- [`windows-timezones` 0.5.1](https://docs.rs/windows-timezones/0.5.1/windows_timezones/)
- [`windows-timezones` upstream manifest and release](https://github.com/kyrias/windows-timezones)
- [`localzone` 0.3.1](https://docs.rs/localzone/0.3.1/localzone/)
- [`exchangelib` all-day calendar response mapping reference](https://ecederstrand.github.io/exchangelib/exchangelib/items/calendar_item.html)
- [Wino Mail Graph calendar synchronization](https://github.com/bkaankose/Wino-Mail/blob/master/src/Wino.Core/Synchronizers/OutlookSynchronizer.cs)
- [Thunderbird Graph calendar provider](https://github.com/thunderbird/thunderbird-desktop/blob/main/calendar/providers/graph/GraphCalendar.sys.mjs)
