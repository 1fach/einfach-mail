# ADR 0018: Reply to one message through Microsoft Graph

- Status: Accepted
- Date: 2026-11-01
- Owners: Einfach Mail maintainers

## Context

The native mail view opens cached messages. The app can also compose a new plain-text message. The next mail action lets users reply to the message they are reading.

Microsoft Graph v1.0 exposes `POST /me/messages/{id}/reply`. The request accepts either a `comment` or a writable `message` body. It rejects a request that includes both. The app can set the body content type to `Text`.

Graph selects recipients from the source message's `replyTo` property when it is present. It does not require the app to guess from `from`. Graph saves the reply in Sent Items and returns `202 Accepted` without a response body. The delegated least-privileged permission is `Mail.Send`.

The `createReply` endpoint creates a remote draft and requires `Mail.ReadWrite`. A later request must send that draft. No test tenant is available, so local work uses synthetic tests only.

The existing send workflow writes one encrypted, body-free marker per account before submission. It blocks another send and account removal until the user resolves an uncertain result. Replies must share that marker and guard. A separate operation table could let a new send race with a reply.

## Decision

- Add a native Reply action to the currently opened cached message. The action opens a keyboard-accessible plain-text composer. Tell users that Graph selects recipients from the source message. Do not accept or invent a recipient, subject, reply-all list, or forward target. Exclude reply-all, forwarding, HTML, attachments, and offline sends.
- Model the source as an account-scoped folder and message reference. Verify that the source remains cached under the requested account before staging or contacting Graph. Make the provider reject a source from another account.
- Submit one Graph v1.0 `POST /me/messages/{id}/reply`. Set `message.body.contentType` to `Text` and put the user's text in `message.body.content`. Omit `comment`, `toRecipients`, and a replacement subject. Graph selects reply recipients and places the reply in Sent Items.
- Request delegated `Mail.Send` through the existing same-account permission renewal flow. Require the user to choose permission review first. Renewal must not submit a reply. Require a separate Send action afterward. Keep tokens within the Microsoft provider.
- Extend the per-account unresolved send record with a tagged target. Store either a new-message recipient and subject or a reply's source folder and message IDs with its optional subject. Migrate existing new-message records without changing their meaning. Do not persist a reply body or tokens. Keep the one-unresolved-operation rule and the existing sign-out and account-removal guard.
- Persist the body-free marker before the Graph POST. Treat only `202 Accepted` as accepted for processing, not delivered. Clear the marker after a definite result. If the response may have been lost, or marker cleanup fails after acceptance, retain the marker and do not retry. On restart, require the user to inspect Sent Items and explicitly resolve the marker before another send or sign-out.
- Keep Graph JSON inside the existing provider boundary. Use the existing domain identifiers and Serde. The reply action needs no new general-purpose library.
- Add synthetic Graph, application, SQLCipher migration/restart, and headless native UI coverage. Keep real consent, `Reply-To` behavior, and Sent Items verification in issue #30. Do not make a live request or launch the GUI as part of implementation.

## User flow

Open a cached message and choose **Reply**. Enter plain text in the composer. Review the delegated `Mail.Send` permission if Microsoft requests it. Permission approval does not send the reply. Choose **Send** to submit it.

The operation marker stores the source reference and operation time. It does not store the reply body.

## Types and ownership

- `crates/domain` owns the account-scoped reply source, in-memory reply draft, and tagged body-free send target. Its debug output redacts message content and source identifiers.
- `crates/application` verifies that the source is cached, reserves the account's send operation, stages and resolves the shared pending marker, and calls one provider operation.
- `crates/providers/microsoft-graph` encodes the opaque source ID as one URL path segment, serializes the Graph `message.body` JSON, and classifies the exact `202` result. The provider does not retry this POST.
- `crates/persistence` migrates the existing pending marker table to store a validated target variant, preserving old new-message rows. Source IDs remain under the existing account SQLCipher cache boundary.
- `apps/desktop` presents Reply from the opened message and owns the native composer state and consent disclosure. It calls application workflows only.

## Alternatives

| Option | Benefit | Cost or risk |
| --- | --- | --- |
| One Graph `reply` POST with a `Text` message body | Preserves the source message's reply routing and thread semantics with one remote write and the already accepted `Mail.Send` grant. | A lost response remains ambiguous. The user must reconcile it, and the source must still exist in Exchange Online. |
| `createReply`, edit the generated draft, then send it | Supports an editable Graph draft and can return a server message ID before send. | Adds `Mail.ReadWrite`, multiple remote writes, draft cleanup, and an ambiguous draft-creation outcome that may lose its ID. |
| Build a new `sendMail` request from cached `from` and a synthesized subject | Reuses the current new-message request. | Can ignore `Reply-To` and does not preserve Graph's reply/thread behavior. |
| Put text in Graph's `comment` property | Short request body. | Leaves the message body representation and generated quoting to Graph. The explicit `message.body` form makes the `Text` body clear in the provider contract. |
| Build a MIME reply locally | Allows full control over headers and formatting. | Adds MIME generation and encoding even though Graph's JSON reply endpoint handles source recipients and accepts a plain-text body. |

## Consequences

- New messages and replies share one account-level operation reservation and encrypted unresolved marker. A reply cannot be replayed automatically after a restart or timeout.
- The tagged marker must preserve existing in-flight new-message rows during the schema migration. Account removal remains blocked while either send target is unresolved.
- Microsoft selects reply recipients from the source message. The local UI must not imply that the displayed `From` address is always the actual reply destination.
- Graph acceptance is not a delivery receipt. Synthetic tests verify local request construction and state transitions. Issue #30 tracks tenant evidence.
- No new crate is required. The existing typed IDs, Serde boundary, SQLCipher store, Graph transport, and headless GPUI harness cover this action.

## Sources

- [Microsoft Graph `message: reply` v1.0](https://learn.microsoft.com/en-us/graph/api/message-reply?view=graph-rest-1.0)
- [Microsoft Graph `message: createReply` v1.0](https://learn.microsoft.com/en-us/graph/api/message-createreply?view=graph-rest-1.0)
- [Microsoft Graph permissions reference](https://learn.microsoft.com/en-us/graph/permissions-reference?view=graph-rest-1.0)
- [Microsoft Entra incremental consent](https://learn.microsoft.com/en-us/entra/identity-platform/consent-types-developer)
- [ADR 0017: Send one plain-text message](0017-safe-plain-text-mail-sending.md)
- [Issue #27: Compose and send one plain-text message](https://github.com/1fach/einfach-mail/issues/27)
- [Issue #29: Reply to one cached message with plain text](https://github.com/1fach/einfach-mail/issues/29)
- [Issue #30: Verify message reply in a Microsoft 365 test tenant](https://github.com/1fach/einfach-mail/issues/30)
