# ADR 0057: Propose read-only EWS calendar snapshots

- Status: Proposed
- Date: 2026-11-09
- Owners: Einfach Mail maintainers

## Context

Issue [#266](https://github.com/1fach/einfach-mail/issues/266) extends the Linux on-premises Exchange account from read-only mail to read-only primary-calendar views. Accepted [ADR 0053](0053-linux-read-only-exchange-ews.md) leaves EWS calendars unavailable; this proposal changes that scope while keeping Exchange Online on Graph and EWS writes unavailable.

The existing Graph calendar workflow uses fixed-range `calendarView/delta` queries and opaque per-view cursors. EWS has different semantics. Microsoft documents `FindItem` with `CalendarView` as the range query that expands recurrence occurrences and exceptions. `SyncFolderItems` does not apply calendar recurrence logic. `CalendarView` exposes a result limit but no indexed offset. Indexed paging uses another view type and does not expand recurrence. The `FindItem` response includes `IncludesLastItemInRange`, which can identify an incomplete result.

The domain view already contains explicit UTC instant bounds and the user's local date bounds. EWS `CalendarView` accepts UTC `StartDate` and `EndDate`, so the request does not need to translate the local IANA zone into a Windows ID. EWS calendar items expose `Start`, `End`, `IsAllDayEvent`, `StartTimeZone`, and `EndTimeZone`. The EWS response time values include an explicit UTC or numeric offset. Recover all-day civil dates in each event's original start/end zone rather than treating its UTC date as a calendar date.

The workspace already uses Thunderbird's `ews` 0.1.2 crate for EWS mail. It provides `FindItem`, `View::CalendarView`, and the response's range-completeness metadata, but its typed `Message` has no calendar `Start`, `End`, all-day, location, or time-zone fields. Existing `quick-xml` and Serde dependencies can map only these provider-specific fields without adding a general XML implementation. Wino Mail's calendar code uses Microsoft Graph `CalendarView.Delta`; it is not an EWS calendar implementation. Thunderbird's calendar provider is also Graph-based.

## Decision

Propose adding Linux read-only EWS primary-calendar views in M8:

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
- The application provider and cache contracts must represent a complete snapshot separately from a Graph delta page. The current delta cursor and reset behavior remain Graph-specific.
- The EWS response mapper is narrow provider code because the selected Rust EWS crate has no typed calendar item fields. `quick-xml`, `serde`, `ews`, `jiff`, and the existing HTTP transport remain the starting points. The dependency review recommends `windows-timezones` 0.5.1 for Windows-ID-to-tzdb mapping: it has no default runtime dependencies and exposes the mapping directly. Unicode-DFS-2016 permits use and distribution when its notices are preserved; include that notice in third-party attributions. Its manifest does not declare an MSRV, so compile it on the workspace Rust 1.95 before adding it.
- `localzone` 0.3.1 offers a similar bidirectional CLDR map under Apache-2.0, but its latest release is from April 2024 and it is not the preferred candidate. Neither library is added in this proposal.
- Synthetic fixtures can prove request construction, field mapping, recurrence occurrence mapping, time-zone conversion, completeness checks, limits, cache replacement, and rollback. No Exchange Server or sanitized calendar response recording is available, so live interoperability remains unverified and must not be claimed.

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
