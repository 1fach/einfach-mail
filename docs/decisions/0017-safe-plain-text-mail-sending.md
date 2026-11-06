# ADR 0017: Send one plain-text message through Microsoft Graph

- Status: Accepted
- Date: 2026-11-01
- Owners: Einfach Mail maintainers

## Context

The mail view currently reads and caches the signed-in account's messages. M4 needs one explicit, native compose-and-send action. The Graph provider already uses delegated OAuth, an account-checked permission-renewal flow, a non-retrying write transport, and a SQLCipher cache isolated to one account. There is no Microsoft 365 test tenant, so local verification must use synthetic responses and must not claim live sending or consent.

Microsoft Graph v1.0 exposes `POST /me/sendMail` for a signed-in user. Its delegated least-privilege permission is `Mail.Send`; `Mail.ReadWrite` does not include sending. Graph responds with `202 Accepted` and no body. This confirms acceptance for processing only. The API has no caller-supplied idempotency key and does not return a message ID. A lost response can therefore leave the app unable to determine whether the message was accepted.

The app must not request `Mail.Send` on first connection. It can request additional delegated permissions through the same-account authorization-code renewal path. The user must explicitly start that browser flow after seeing a clear disclosure. A denied, cancelled, mismatched-account, or failed credential update must leave the existing account session active.

Recipient input is one bare address rather than a full RFC 5322 header. The project must use a maintained parser instead of implementing address grammar. A user can submit a send operation, and the app must preserve enough encrypted, account-scoped metadata to warn after a restart without persisting the message body.

## Decision

- Add one native GPUI composer for one recipient, a subject, and a plain-text body. Exclude CC, BCC, attachments, HTML, signatures, drafts, reply, forward, scheduling, shared-mailbox sending, and offline send queues.
- Use Graph v1.0 `POST /me/sendMail` with one `toRecipients` entry, a `Text` body, and `saveToSentItems: true`. Do not create a draft first. Request only delegated `Mail.Send` when the user explicitly chooses the send-permission review action; keep it out of initial sign-in. Explain that Microsoft grants the app permission to send mail as the signed-in user, and that the app sends only after the user presses Send.
- Reuse same-account permission renewal with a typed permission request. Confirm the returned account identity before replacing the stored refresh token or active session. Do not automatically retry a message after permission renewal; keep the composer content in memory and require the user to press Send again.
- Parse a single bare recipient at the domain boundary with `addr-spec` 0.10.4. It directly parses RFC 5322 `addr-spec`, supports RFC 6532 UTF-8 and NFC normalization, declares Rust 1.81, and has an Apache-2.0 license. The upstream repository had recent activity on 2026-10-25. Its parser uses internal `unsafe` storage code; this project calls only its safe `FromStr` API and adds no unsafe code. Keep the parser out of Graph and persistence wire types. Reject `email-address-parser` 2.0.2 because its `quick-xml` 0.39.4 build dependency is affected by RUSTSEC-2026-0194 and RUSTSEC-2026-0195, and its caret requirement excludes patched 0.41.0. `email_address` 0.2.9 has a smaller API but its last stable release was 2024-07-31. Do not add `lettre` or `mail-builder`; Graph accepts a JSON message object and the app does not use SMTP or MIME construction.
- Before the Graph request, persist one operation marker in the signed-in account's encrypted SQLCipher cache. Store an operation ID, recipient, subject, and creation time; never store the body, access token, or refresh token. Enforce at most one unresolved send operation per account. If staging fails, do not make a network request.
- Treat only a valid `202 Accepted` as Graph acceptance. Remove the matching marker after acceptance. If removing it fails, report that Graph accepted the message while retaining the marker to prevent another send. Treat explicit Graph rejection statuses as rejected, retain the composer, and clear the marker only after the definitive rejection is recorded locally. Treat transport failure after dispatch, timeout, 5xx, 408, or any unexpected success status as an unknown outcome and keep the marker. Never automatically retry an unknown send.
- On restart, any remaining marker is unresolved; never infer that the request was not sent and never replay it. Show its recipient and subject from the encrypted cache, direct the user to Sent Items, and block another send until the user explicitly resolves it. If the user chooses to send a new message after checking, warn that a duplicate is possible and require a separate confirmation.
- Refuse account sign-out while an unresolved send marker exists, because sign-out removes the encrypted cache that holds the warning. Apply this guard when the mail view, calendar view, or no account view is open. If an existing cache cannot be inspected, preserve it and stop sign-out rather than deleting a marker the app cannot read.
- Coordinate pending-marker staging with cache removal across app processes using the existing per-account `mail-cache.lock`. Keep `std::fs::File::lock` as the Linux advisory lock, and acquire it through `blocking::unblock` so waiting does not block the async executor. Record the database file identity when a cache handle opens, then reject send staging if that handle points to a database that has since been removed or replaced. This makes removal-first and stage-first races safe without treating an in-memory reservation as cross-process coordination.
- Share mail-folder and calendar-view operation reservations across the account session. Reject account-cache removal while either workflow is active so sign-out cannot delete the database during a calendar page request or commit.
- Keep provider, workflow, persistence, and headless GPUI fixtures synthetic. Do not launch a GUI or call the live Graph send endpoint. Track tenant proof separately in issue #28.

Direct `sendMail` is selected over draft-first delivery. A draft would provide a message ID that could be queried after a lost send response, but creating the draft is itself a non-idempotent remote write that can time out without returning its ID. Draft-first also introduces an extra remote operation, draft cleanup states, and another permission surface. For this single action, the encrypted local marker and explicit user reconciliation prevent silent duplicate sends with fewer remote writes. The app does not claim it can automatically determine the outcome of an ambiguous request.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Direct Graph `sendMail` with a durable unresolved marker | One remote write, one added permission, and an explicit guard against silent resend after uncertain outcomes. | Graph returns no message ID; the user must check Sent Items after an ambiguous response. |
| Create a Graph draft, then send it by immutable ID | A known draft ID can be queried after an uncertain send, and moving a message within a mailbox preserves its immutable ID. | Draft creation can itself time out without an ID; adds a remote state machine, cleanup cases, and Mail.ReadWrite use for a second write. |
| Automatically resend after a timeout or 5xx | Can appear to recover without user action. | Graph provides no idempotency key for `sendMail`; the first request may have been accepted, creating duplicates. |
| Build or regex-validate recipient syntax in the app | Avoids a direct dependency. | Reimplements a general-purpose RFC parser at an untrusted boundary and will mishandle valid or invalid edge cases. |
| Use `emailaddr` 1.0.0 | MIT/Apache-2.0, Rust 1.86, strict address types, SMTP length checks, and IDNA normalization. | Its broader SMTP and IDNA policy exceeds this input's `addr-spec` contract, its default feature adds `idna` and `thiserror`, and its implementation also uses internal unsafe casts. Its current release does not yet establish a maintenance cadence. |
| Use `email-address-parser` 2.0.2 | MIT license, strict RFC 5322/6532 address parsing, and no display-name support. | Its `quick-xml` 0.39.4 build dependency is affected by two RustSec advisories, and the declared caret range excludes patched 0.41.0. Its 3.0 release remains a release candidate. |
| Use `email_address` 0.2.9 | Small API, UTF-8 support, MIT license, and no required runtime dependencies when Serde is disabled. | Last stable release was 2024-07-31 and it does not normalize equivalent address forms. |
| Use `lettre` or `mail-builder` to construct a message | Rich MIME and RFC message-building APIs. | Graph accepts a typed JSON message; SMTP/MIME construction is outside this feature and increases dependency surface. |

## Consequences

- The application and domain gain validated recipient, draft, send-operation, and outcome types. Address parsing stays at the external/user-input boundary.
- The account workflow can renew consent for a typed additional delegated permission while preserving the active identity on failure.
- The Graph adapter owns Graph JSON serialization and strict response classification. It never retries `sendMail`, including after a 401 refresh, 429, or transport error.
- The encrypted mail cache gains a one-per-account unresolved-send record. It stores no body. Account removal is blocked until the marker is resolved, so deleting the cache cannot silently erase the warning.
- The composer retains its message in memory for definitive permission errors. An app restart discards the unsent message body but preserves the warning marker if dispatch may have occurred.
- `202 Accepted` means Graph accepted the request for processing; it is not delivery confirmation. Issue #28 remains tenant-gated for delegated consent, the real request/response path, and Sent Items reconciliation.
- Local tests establish parser behavior, request serialization, status classification, permission scoping, restart handling, account isolation, and native UI state. They do not establish live Entra or Exchange Online behavior.

## Sources

- [Microsoft Graph `sendMail` v1.0](https://learn.microsoft.com/en-us/graph/api/user-sendmail?view=graph-rest-1.0)
- [Microsoft Graph permissions reference](https://learn.microsoft.com/en-us/graph/permissions-reference?view=graph-rest-1.0)
- [Microsoft Entra incremental consent](https://learn.microsoft.com/en-us/entra/identity-platform/consent-types-developer)
- [`addr-spec` 0.10.4 API](https://docs.rs/addr-spec/0.10.4/addr_spec/)
- [`addr-spec` source and release history](https://github.com/mathematic-inc/addr-spec-rs)
- [`emailaddr` 1.0.0 API](https://docs.rs/emailaddr/1.0.0/emailaddr/)
- [`emailaddr` source](https://github.com/al8n/emailaddr)
- [`email-address-parser` 2.0.2 API](https://docs.rs/email-address-parser/2.0.2/email_address_parser/)
- [`email-address-parser` source and release history](https://github.com/Sayan751/email-address-parser/releases)
- [`email_address` 0.2.9 metadata and release history](https://docs.rs/crate/email_address/0.2.9)
- [`addr-spec` 0.10.4 API and implementation notes](https://docs.rs/addr-spec/0.10.4/addr_spec/)
- [Rust dependency review: mail address validation](../project/dependency-review.md#plain-text-message-recipient-validation)
- [ADR 0006: Graph HTTP transport](0006-graph-http-transport.md)
- [ADR 0008: Identity token boundary](0008-identity-token-boundary.md)
- [ADR 0009: Account-scoped domain identifiers](0009-account-scoped-domain-identifiers.md)
- [ADR 0015: Idempotent message read-state update](0015-idempotent-message-read-state-update.md)
- [Issue #27: Compose and send one plain-text message](https://github.com/1fach/einfach-mail/issues/27)
- [Issue #28: Verify plain-text send in a Microsoft 365 test tenant](https://github.com/1fach/einfach-mail/issues/28)
