# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Added I/O-free To Do task list coroutines `MsgraphTodoListsList`, `MsgraphTodoListGet`, `MsgraphTodoListCreate`, `MsgraphTodoListUpdate`, `MsgraphTodoListDelete` and `MsgraphTodoListsDelta`, over `MsgraphTodoList`.
- Added I/O-free To Do task coroutines `MsgraphTodoTasksList`, `MsgraphTodoTaskGet`, `MsgraphTodoTaskCreate`, `MsgraphTodoTaskUpdate`, `MsgraphTodoTaskDelete` and `MsgraphTodoTasksDelta`, over `MsgraphTodoTask`.

  A task's and a list's `etag` (`@odata.etag`) is their revision. Linked resources come inline, checklist items when expanded.
- Added I/O-free `MsgraphChecklistItemsList` coroutine.
- Added the iCalendar projection of a To Do task, `MsgraphTodoTask::to_ical`, `from_ical`, `create_from_ical`, `update_from_ical` and `stashed_uid` (requires `ical` feature).

  Dates are told in a zone the caller passes, and every line without a Graph field is stashed as events do, read back through `MSGRAPH_TODO_TASK_STASH_EXPAND`.
- Added the To Do methods of `MsgraphClientStd`.

### Fixed

- Fixed `MsgraphEvent::to_ical` scattering the lines of a stashed alarm, an alarm Graph's reminder cannot hold, across the event.

## [0.7.0] - 2026-11-04

### Added

- Added `MsgraphBatchResponse::bytes`, decoding the base64 JSON string Graph answers for a non-JSON inner body (such as `$value` MIME), and `MsgraphBatchResponse::retry_after`, the inner `Retry-After` delay in seconds.

### Changed

- Bumped io-http to 0.6, ical-rs to 0.7 and vcard-rs to 0.6, whose `Http11SendError`, `IcalParseError` and `VcardParseError` `MsgraphSendError::Send`, `MsgraphEventIcalError::Parse` and `MsgraphContactVcardError::Parse` carry. A Graph response over 64 KiB of head or 256 MiB of body now fails with `MsgraphSendError::Send`, and a vCard line built from Graph is folded at 75 octets. **Breaking.**
- Raised the minimum supported Rust version from 1.88 to 1.89.
- Replaced the base64 crate with [base64-codec](https://crates.io/crates/base64-codec) for MIME sends, attachment content and `MsgraphBatchResponse::bytes`.

  It runs on the CPU token the application installs with `simd_cpu::SimdCpu::install_detected`, the portable baseline when none is. `bytes` accepts line breaks, blanks and missing padding, and still refuses any other byte outside the alphabet.

## [0.6.0] - 2026-10-10

### Added

- Added `expand` (`$expand`) to `MsgraphMessagesListParams` and `MsgraphMessagesDeltaParams`, and `MsgraphMessage::single_value_extended_properties`, filled when the request expands them (such as `Integer 0x0E08`, the MAPI message size). **Breaking** for a struct literal without `..Default::default()`.

## [0.5.0] - 2026-10-09

### Added

- Added `MsgraphEvent::instance_update_from_ical` (`ical` feature): the update body of the Graph instance of one occurrence, from a document holding that occurrence alone (an override, or the series' component carried to it to revert an exception) against one holding it as Graph does, `None` when nothing differs. The recurrence and the stash stay on the master. `from_ical` still writes the master alone.
- Added `MsgraphEvent::is_online_meeting`. `from_ical` reads `X-PIMDIR-ONLINE-MEETING:TRUE` (pimdir STORAGE Annex B.1) as `isOnlineMeeting: true`, so Graph creates an online meeting with the calendar's default provider (Teams); `update_from_ical` sends it only when the base did not ask already. The property is never stashed.
- Added page size preference (`Prefer: odata.maxpagesize={n}`): `MsgraphSend::max_page_size` (and `MsgraphSend::header`), and a `max_page_size` builder on `MsgraphMessagesDelta`, `MsgraphContactsDelta`, `MsgraphEventsDelta`, `MsgraphMessagesList`, `MsgraphContactsList` and `MsgraphEventsList`. Graph forgets it across pages, so it is chained on every `from_link` too. Client: `messages_delta_with_params`, `{messages,contacts,events}_delta_from_link_with_page_size`, `contacts_delta_with_page_size`, `events_delta_with_page_size`.
- Added `MsgraphMessagesDeltaParams` (`$select`, `$filter` on `receivedDateTime`, page size) and `MsgraphMessagesDelta::with_params`.

### Changed

- Bumped ical-rs to 0.6, whose `IcalParseError` `MsgraphEventIcalError::Parse` carries. Lines built from Graph are folded at 75 octets. **Breaking.**
- `from_ical` consumes EXDATE instead of stashing it: Graph mints the EXDATEs from `cancelledOccurrences`, which only the deletion of an instance writes, so a stashed copy doubled them on read and outlived an occurrence Graph never removed.
- `to_ical` mints the join link of an online meeting as a standard `CONFERENCE;VALUE=URI;FEATURE=AUDIO,VIDEO` (RFC 7986 5.11) instead of `X-MICROSOFT-SKYPETEAMSMEETINGURL`. `from_ical` drops both, the server value staying authoritative.

## [0.4.7] - 2026-10-05

### Added

- Added `MsgraphEvent::recurrence_id_of` (`ical` feature): the RECURRENCE-ID value the projection gives the occurrence of a series master that originally started at an instance's `originalStart`, so a consumer told an occurrence by its RECURRENCE-ID finds its Graph instance.
- Added `MsgraphMessage::change_key`, the version of the message that every edit moves.

### Changed

- `to_ical` and `to_ical_series` project an invitation Exchange took in by mail under the organizer's UID: its `iCalUId` is a global object id that wraps that UID (the `vCal-Uid` form), which is now unwrapped (`ical::original_uid`). A global object id Exchange minted itself, with no UID inside, stays as it is. A store keyed on the UID sees such events change UID once.

## [0.4.6] - 2026-10-04

### Added

- Added `MsgraphEvent::online_meeting` (`onlineMeeting.joinUrl`, read-only), selected by `MSGRAPH_EVENT_ICAL_SELECT`. `to_ical` mints the join link of an online meeting as `X-MICROSOFT-SKYPETEAMSMEETINGURL`, the property Outlook writes, so a Teams link reaches the iCalendar document; `from_ical` drops it.

## [0.4.5] - 2026-10-03

### Added

- Added `MsgraphPatternedRecurrence::bounds` (`ical` feature): the dates a series spans, following its range type. Graph fills the `endDate` of a `numbered` or `noEnd` range with `0001-01-01`, so reading `endDate` alone gave an instances window ending before it starts.

### Changed

- Documented that Graph returns `cancelledOccurrences` only on a read of a series master by id, never in a listing.

## [0.4.4] - 2026-10-02

### Fixed

- Fixed `update_from_ical` patching an event's body to `null`, which Graph refuses (HTTP 400, "The body of the item is invalid"): Graph reads back an empty HTML body, so any edit of an event without DESCRIPTION, or one clearing it, failed. A missing DESCRIPTION is now an empty text body, and bodies compare by their text.

## [0.4.3] - 2026-10-02

### Fixed

- Fixed `to_ical` writing an event in UTC: Graph answers a read in UTC, so a series created in Europe/Paris read back anchored on UTC and drifted by an hour at each DST change once expanded. `MSGRAPH_EVENT_ICAL_SELECT` now selects `originalStartTimeZone` and `originalEndTimeZone`, and the projection tells a UTC start and end in that zone when the database knows it.

- Fixed `from_ical` dropping an edited DESCRIPTION: a document read from Graph carries Exchange's HTML body as `X-ALT-DESC` beside the text, and the HTML always won, so `update_from_ical` saw no change and left the edit out of the patch. The HTML now wins only while it says what the text says.

- Documented that `to_ical_series` needs its exceptions read with `MSGRAPH_EVENT_ICAL_SELECT`: Graph's default instance listing leaves out `originalStart`, and an exception without it gets no RECURRENCE-ID.

## [0.4.2] - 2026-10-01

### Added

- Added the `ical` feature: `MsgraphEvent::to_ical`, `to_ical_series`, `from_ical`, `create_from_ical` and `update_from_ical` project an event, or a series with its exceptions, onto an iCalendar document and back.

  The recurrence maps onto an RRULE both ways, a rule Graph cannot hold being refused; cancelled occurrences become EXDATEs and exceptions VEVENTs with a RECURRENCE-ID. Windows zone names read as their CLDR IANA counterpart and every zone gets a VTIMEZONE, through ical-rs's `tzdb` feature. The UID and every unmanaged line ride a stash extended property, read back with `MSGRAPH_EVENT_STASH_EXPAND`. Only the series master is written back.

- Added `MsgraphEvent::cancelled_occurrences`.

- Added calendars and events: `calendars` (list, get, create, update, delete) and `events` (list, get, create, update, delete, the instances of a series, the calendar view and its delta), with `MsgraphCalendar`, `MsgraphEvent` and the recurrence, attendee and time types, and a client method each.

  An event's times keep the zone Graph returns, a Windows or an IANA name; the calendar view and its delta expand series within a window, the events listing returns series masters.

- Added `MsgraphContact::stashed_uid`, the vCard UID a contact's stash carries, for a sync engine to check that a write kept it.

### Changed

- The vCard UID rides the stash, so a contact keeps the UID it was written with. **Behaviour change.**

  It used to be dropped on write and minted from the Graph id on read, which gave one person two identities across a sync. A contact with no stashed UID, created by Graph itself or by an earlier version, still reads back with one minted from its Graph id.

## [0.4.1] - 2026-10-01

### Added

- Added the `vcard` feature: `MsgraphContact::to_vcard`, `from_vcard`, `create_from_vcard` and `update_from_vcard` project a Graph contact onto a vCard 4.0 document and back.

  Graph-only fields ride as read-only `X-MSGRAPH-*` properties, every other line round-trips through a stash extended property read back with `MSGRAPH_CONTACT_STASH_EXPAND`. The projection moved here from Cardamum, stash id included, so contacts Cardamum already stashed still read back.

## [0.4.0] - 2026-09-29

### Added

- Added `MsgraphClientStdConnectOptions::proxy`, tunnelling the connection through a SOCKS5 or HTTP proxy.

## [0.3.1] - 2026-09-29

### Added

- Added JSON batching (`POST /$batch`) and the matching `batch` client method.

### Removed

- Removed the thiserror dependency. Error messages, sources and `From` conversions are unchanged.

## [0.3.0] - 2026-08-15

### Added

- Added `MsgraphContactsDelta::from_link` and the matching `contacts_delta_from_link` client method, resuming a contacts delta round from a saved link.

### Changed

- Bumped pimalaya-stream to 0.3. **Behaviour change.**

  The transport now retries a stream reporting it is not ready for up to a minute, then fails with `TimedOut`, and a read deadline stops a silent server from blocking forever. Consumers must move to the same `Tls` type.

- Bumped io-http to 0.5.
- Raised the MSRV from 1.87 to 1.88.

## [0.2.2] - 2026-08-06

### Added

- Added the messages delta operation (`v1::rest::users::messages::delta`) and the matching `messages_delta` and `messages_delta_from_link` client methods.

## [0.2.1] - 2026-07-25

### Added

- Added the `schemars` feature, deriving `JsonSchema` on the mail output types. It is off by default and stays `no_std`.

## [0.2.0] - 2026-07-16

### Changed

- Moved each resource type into its resource module, dropping the internal `types` submodules.

  Entity types keep their path. `MsgraphField` moved to `v1::field`, and operation companions (`Msgraph*ListResponse`, the contacts delta types) moved into their operation module.

## [0.1.0] - 2026-07-15

### Added

- Added the I/O-free coroutine core for Microsoft Graph v1.0: the `MsgraphCoroutine` contract, the `MsgraphSend` HTTP/JSON primitive and the OData query serializer.
- Added the mail surface: mail folders, messages, attachments and the sendMail action.
- Added the contacts surface: contact folders and contacts, with delta.
- Added `MsgraphClientStd` (`client` feature), a std blocking client with a `connect` constructor behind the TLS features.

[unreleased]: https://github.com/pimalaya/io-msgraph/compare/v0.7.0..HEAD
[0.7.0]: https://github.com/pimalaya/io-msgraph/compare/v0.6.0..v0.7.0
[0.6.0]: https://github.com/pimalaya/io-msgraph/compare/v0.5.0..v0.6.0
[0.5.0]: https://github.com/pimalaya/io-msgraph/compare/v0.4.7..v0.5.0
[0.4.7]: https://github.com/pimalaya/io-msgraph/compare/v0.4.6..v0.4.7
[0.4.6]: https://github.com/pimalaya/io-msgraph/compare/v0.4.5..v0.4.6
[0.4.5]: https://github.com/pimalaya/io-msgraph/compare/v0.4.4..v0.4.5
[0.4.4]: https://github.com/pimalaya/io-msgraph/compare/v0.4.3..v0.4.4
[0.4.3]: https://github.com/pimalaya/io-msgraph/compare/v0.4.2..v0.4.3
[0.4.2]: https://github.com/pimalaya/io-msgraph/compare/v0.4.1..v0.4.2
[0.4.1]: https://github.com/pimalaya/io-msgraph/compare/v0.4.0..v0.4.1
[0.4.0]: https://github.com/pimalaya/io-msgraph/compare/v0.3.1..v0.4.0
[0.3.1]: https://github.com/pimalaya/io-msgraph/compare/v0.3.0..v0.3.1
[0.3.0]: https://github.com/pimalaya/io-msgraph/compare/v0.2.2..v0.3.0
[0.2.2]: https://github.com/pimalaya/io-msgraph/compare/v0.2.1..v0.2.2
[0.2.1]: https://github.com/pimalaya/io-msgraph/compare/v0.2.0..v0.2.1
[0.2.0]: https://github.com/pimalaya/io-msgraph/compare/v0.1.0..v0.2.0
[0.1.0]: https://github.com/pimalaya/io-msgraph/compare/root..v0.1.0
