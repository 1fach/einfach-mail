# ADR 0058: Gate EWS mail actions by operation

- Status: Accepted
- Date: 2026-11-09
- Owners: Einfach Mail maintainers

## Context

The EWS account currently exposes read-only mail. `AccountCapabilities` uses one `MailAccess` value, and the desktop treats `ReadWrite` as permission for every mail action. That gate cannot describe an EWS account that supports one action while rejecting sends, replies, flags, moves, and deletes.

The application already has a provider-neutral read-state workflow with cache recovery. EWS represents the state as `Message.IsRead` and supports updates through `UpdateItem`. The pinned `ews` crate has typed update and message structures. Its `UpdateItem` request type does not expose the `SuppressReadReceipts` attribute.

Microsoft documents that setting `IsRead` to true sends a read receipt when the message requests one. The `SuppressReadReceipts` attribute prevents that behavior and was introduced in Exchange Server 2013 SP1. The documented successful `UpdateItem` response returns the updated item ID and change key, not the `IsRead` value. The pinned Thunderbird desktop subtree has no EWS mail `UpdateItem` workflow, and Wino Mail uses Graph for its provider path. Use Microsoft's EWS documentation and the `ews-rs` types for this protocol decision. There is no Exchange server or sanitized live response available for verification.

## Decision

Make setting message read state the first EWS mail action. It is idempotent, has an existing application workflow and cache path, and does not need message composition or folder movement.

Replace the all-or-nothing `MailAccess` write gate with a provider-neutral `MailCapabilities` value that reports supported operations individually. Define a closed `MailAction` enum for read-state updates, follow-up flags, send, reply, move, and delete, and store its variants with `enumset`. Graph retains its current actions. EWS advertises only `SetReadState`; the other EWS actions remain hidden and return `UnsupportedOperation` at the provider boundary.

Keep Microsoft Graph mail-write permission review as a separate account capability. A provider may support an individual mail action without using Microsoft's Graph consent flow; such an account must not expose or request that flow.

Expose EWS read-state updates only after the connection establishes that the server can suppress read receipts. Send `UpdateItem` with `MessageDisposition=SaveOnly`, `SuppressReadReceipts=true`, and a `SetItemField` update for `message:IsRead`. Do not send this write when the server version is unknown or predates Exchange Server 2013 SP1. Keep such accounts read-only.

Use the newest maintained `ews` crate release that supports the request. The pinned `ews` 0.1.2 and its upstream `main` still do not expose `SuppressReadReceipts` as of 2026-11-09. Keep the typed `UpdateItem` serializer and use the provider's existing `quick-xml` dependency to add that single attribute to the serialized `UpdateItem` start element. Do not fork or vendor the crate. Validate that the serializer produced exactly one expected operation element before adding the attribute; fail before sending if its shape differs. Recheck upstream before replacing this narrow adapter with a crate API.

Read the SOAP `ServerVersionInfo` header from the authenticated folder-hierarchy response during connect and restore. Gate the capability on the complete numeric product tuple `(MajorVersion, MinorVersion, MajorBuildNumber, MinorBuildNumber)` being at least `(15, 0, 847, 32)`, the Exchange Server 2013 SP1 baseline. Missing, malformed, or overflowing fields mean unknown and leave the account read-only. Do not use `RequestServerVersion` or `ServerVersionInfo.Version` as product-version evidence: the former selects the EWS schema requested, while the latter is not the numeric product build. Check the version again on the fresh `GetItem` response immediately before a write, and send `UpdateItem` only on the authenticated connection whose response supplied that evidence. If the transport must establish or re-establish NTLM during the write, stop before replaying the write body; a later read or account restore can establish a connection and version again.

For each requested read-state update, fetch the current item by its exact, case-sensitive EWS ID. Require an explicit `NoError` response code, exactly that ID, and a non-empty `ChangeKey`; treat a contradictory response class and code as invalid. Do not add EWS change keys to provider-neutral domain types or persistence. Send typed `UpdateItem` with `MessageDisposition=SaveOnly`, `ConflictResolution=NeverOverwrite`, the fresh ID and change key, `SuppressReadReceipts=true`, and only a `SetItemField` update to `message:IsRead`. `NeverOverwrite` makes a concurrent change between `GetItem` and `UpdateItem` a server-rejected conflict instead of overwriting it.

Accept only one `ResponseClass::Success` containing exactly one item whose case-sensitive ID matches the requested ID and whose returned change key is present and non-empty. Do not require the returned change key to differ: setting an already-current state may be a no-op. A valid explicit EWS `ResponseClass::Error` with a non-`NoError` response code is a known rejection. Treat a warning, contradictory or unparseable response code, missing or extra response messages/items, malformed or mismatched confirmation, and any transport failure after the authenticated write may have been dispatched as `MessageUpdateOutcomeUnknown`. Never retry that outcome. This lets the existing application workflow retain its folder sync marker until a complete sync reconciles cached state. Errors proven to occur before write dispatch remain ordinary provider errors.

Do not change Graph behavior or enable any other EWS mail or calendar action in this decision.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Per-operation mail capabilities | Represents Graph's current action set and EWS's smaller set without provider-specific UI checks. | Requires updating each existing mail control and command to ask about its operation. |
| Add another `MailAccess` level | Small initial diff for one EWS action. | Cannot represent independent combinations once EWS adds another write; an access-level variant per combination becomes hard to maintain. |
| Keep `ReadWrite` and special-case EWS in the desktop | Avoids changing the domain profile immediately. | Duplicates provider knowledge in the UI and enables unsupported actions whenever the broad gate is missed. |
| Start with follow-up flags | Avoids read-receipt behavior. | The EWS `Flag` contains status and dates. Updating the object safely requires preserving fields the user did not change. |
| Start with send, reply, or move | Adds a more visible user outcome. | These actions have broader side effects and require more provider-specific request and recovery behavior. |

For the missing `SuppressReadReceipts` request field:

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Add only the missing attribute with the existing XML library | Keeps `ews` typed request construction, adds no dependency, and changes only the `UpdateItem` boundary. | A small serializer adapter must be guarded by exact element and attribute checks and synthetic XML tests. |
| Fork or vendor `ews` | Makes the attribute part of a typed request struct. | Copies a larger protocol crate and creates a maintenance fork for one field while upstream remains usable for every other operation. |
| Build `UpdateItem` XML by hand | Avoids a serializer post-processing step. | Duplicates the library's namespace and item-update serialization and creates a second SOAP construction path. |

## Consequences

- EWS users can mark messages read or unread only when the provider confirms that receipt suppression is available. Other EWS mail actions remain unavailable.
- The mail view and account view must gate each action from the profile capability. The Graph permission review must remain hidden for an EWS account that supports only read-state updates.
- The EWS provider must handle version evidence and the missing receipt-suppression field before exposing the capability. Synthetic SOAP tests do not establish compatibility with a real Exchange server.
- The EWS write path uses a fresh server `ChangeKey` and fails on a concurrent update. It does not persist provider-specific change keys or overwrite conflicts.
- Read-state support requires numeric Exchange product-version evidence at or above `15.0.847.32` on the authenticated connection. Unknown or older versions remain read-only.
- A warning or incomplete confirmation after write dispatch leaves the application's folder sync marker in place; explicit SOAP errors are known rejections and do not trigger automatic retries.
- Keep issue [#257](https://github.com/1fach/einfach-mail/issues/257) open for live EWS verification. Issue [#267](https://github.com/1fach/einfach-mail/issues/267) tracks this implementation.

## Sources

- [Microsoft: UpdateItem operation](https://learn.microsoft.com/en-us/exchange/client-developer/web-service-reference/updateitem-operation)
- [Microsoft: UpdateItem request and response](https://learn.microsoft.com/en-us/exchange/client-developer/web-service-reference/updateitem)
- [Microsoft: ItemId and ChangeKey](https://learn.microsoft.com/en-us/exchange/client-developer/web-service-reference/itemid)
- [Microsoft: ServerVersionInfo](https://learn.microsoft.com/en-us/exchange/client-developer/web-service-reference/serverversioninfo)
- [Microsoft: RequestServerVersion](https://learn.microsoft.com/en-us/exchange/client-developer/web-service-reference/requestserverversion)
- [Microsoft: Exchange Server build numbers](https://learn.microsoft.com/en-us/exchange/new-features/build-numbers-and-release-dates/)
- [Microsoft: IsReadReceiptRequested](https://learn.microsoft.com/en-us/exchange/client-developer/web-service-reference/isreadreceiptrequested)
- [Microsoft: Email properties and elements in EWS](https://learn.microsoft.com/en-us/exchange/client-developer/exchange-web-services/email-properties-and-elements-in-ews-in-exchange)
- [Thunderbird `ews-rs`](https://github.com/thunderbird/ews-rs)
- [`ews` 0.1.2 `UpdateItem` source](https://docs.rs/crate/ews/0.1.2/source/src/types/update_item.rs)
- [Upstream `ews-rs` `UpdateItem` source](https://github.com/thunderbird/ews-rs/blob/main/ews/src/types/update_item.rs)
- [Graph read-state request and response handling](../../crates/providers/microsoft-graph/src/graph/mail.rs)
