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

Use the newest maintained `ews` crate release that supports the request. If no suitable release exposes receipt suppression, document the library review and add only the missing request field at the provider boundary. Preserve or retrieve the current EWS change key, reject stale updates, validate the successful response's item identity, and never retry an uncertain outcome. Return success only for a successful response to the requested item. The application workflow remains responsible for updating the cache and requiring synchronization after an uncertain outcome.

Do not change Graph behavior or enable any other EWS mail or calendar action in this decision.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Per-operation mail capabilities | Represents Graph's current action set and EWS's smaller set without provider-specific UI checks. | Requires updating each existing mail control and command to ask about its operation. |
| Add another `MailAccess` level | Small initial diff for one EWS action. | Cannot represent independent combinations once EWS adds another write; an access-level variant per combination becomes hard to maintain. |
| Keep `ReadWrite` and special-case EWS in the desktop | Avoids changing the domain profile immediately. | Duplicates provider knowledge in the UI and enables unsupported actions whenever the broad gate is missed. |
| Start with follow-up flags | Avoids read-receipt behavior. | The EWS `Flag` contains status and dates. Updating the object safely requires preserving fields the user did not change. |
| Start with send, reply, or move | Adds a more visible user outcome. | These actions have broader side effects and require more provider-specific request and recovery behavior. |

## Consequences

- EWS users can mark messages read or unread only when the provider confirms that receipt suppression is available. Other EWS mail actions remain unavailable.
- The mail view and account view must gate each action from the profile capability. The Graph permission review must remain hidden for an EWS account that supports only read-state updates.
- The EWS provider must handle version evidence and the missing receipt-suppression field before exposing the capability. Synthetic SOAP tests do not establish compatibility with a real Exchange server.
- Keep issue [#257](https://github.com/1fach/einfach-mail/issues/257) open for live EWS verification. Issue [#267](https://github.com/1fach/einfach-mail/issues/267) tracks this implementation.

## Sources

- [Microsoft: UpdateItem operation](https://learn.microsoft.com/en-us/exchange/client-developer/web-service-reference/updateitem-operation)
- [Microsoft: UpdateItem request and response](https://learn.microsoft.com/en-us/exchange/client-developer/web-service-reference/updateitem)
- [Microsoft: IsReadReceiptRequested](https://learn.microsoft.com/en-us/exchange/client-developer/web-service-reference/isreadreceiptrequested)
- [Microsoft: Email properties and elements in EWS](https://learn.microsoft.com/en-us/exchange/client-developer/exchange-web-services/email-properties-and-elements-in-ews-in-exchange)
- [Thunderbird `ews-rs`](https://github.com/thunderbird/ews-rs)
- [`ews` 0.1.2 `UpdateItem` source](https://docs.rs/crate/ews/0.1.2/source/src/types/update_item.rs)
- [Graph read-state request and response handling](../../crates/providers/microsoft-graph/src/graph/mail.rs)
