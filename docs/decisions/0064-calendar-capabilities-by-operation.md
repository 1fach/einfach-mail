# ADR 0064: Model calendar capabilities by operation

- Status: Accepted
- Date: 2026-11-12
- Owners: Einfach Mail maintainers

## Context

`AccountCapabilities` currently stores one `CalendarAccess` value: unavailable, read-only, or read-write. The desktop treats read-write as support for event creation, editing, and deletion. That works for Microsoft Graph, which implements all three operations. It cannot describe a provider that supports only some operations.

EWS calendar writes need separate request, conflict, and recovery rules. Issue [#389](https://github.com/1fach/einfach-mail/issues/389) plans those rules while keeping EWS calendar access read-only. ADR [0058](0058-per-operation-ews-mail-capabilities.md) already models EWS mail support per operation with `MailAction` and `MailCapabilities`.

## Decision

- Replace the coarse calendar access value with provider-neutral `CalendarAction` and `CalendarCapabilities` domain types. The closed action set covers viewing events, creating events, updating events, and deleting events.
- Use the existing `enumset` dependency in the domain crate. Provide constructors that make it impossible to advertise a write without also advertising calendar viewing. An empty set means unavailable; viewing alone means read-only.
- Have the selected provider advertise only implemented and accepted operations. Graph advertises viewing and its current create, update, and delete support. EWS continues to advertise viewing alone. Do not expose an EWS write until its operation-specific request, response, and recovery rules are accepted and implemented.
- Gate each desktop action from its matching capability. Event creation requires create support. Event fields and saving require update support. The delete action requires delete support. If a provider supports deletion without updates, show the selected event read-only and keep the delete action available. Apply the same checks to keyboard entry points.
- Keep provider methods as the enforcement boundary. A provider must reject unsupported operations even if a caller bypasses the desktop. Tests must check that each account profile's advertised actions match its provider behavior.
- Keep capabilities derived from the connected provider. They do not change the encrypted cache schema or represent Microsoft permissions.

This decision changes the capability model only. It does not enable EWS calendar writes or change the current Graph behavior. Issue [#390](https://github.com/1fach/einfach-mail/issues/390) tracks the domain and desktop migration. Issue #389 retains the EWS request, response, and recovery decisions.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Keep one read-write calendar state and ship EWS create, update, and delete together | Keeps the domain and UI gates small. The user gets a complete set of calendar actions at once. | A safe operation cannot ship while another operation's distinct request or recovery policy remains unfinished. |
| Add provider-specific checks in the desktop | Avoids changing `AccountCapabilities` in the short term. | Duplicates provider knowledge in the UI and lets a missed check expose an unsupported action. |
| Model calendar operations as a capability set | Represents partial provider support in one provider-neutral type and allows safe actions to ship in small increments. | Requires action-specific UI states, including a read-only selected event when delete is supported without update. |

Use the capability set. It follows the accepted mail capability model and keeps provider-specific rules out of the desktop.

## Consequences

- `AccountCapabilities`, provider profile construction, account details, calendar navigation, the event composer, and their tests must move from `CalendarAccess` to operation-level capabilities.
- Graph continues to advertise all current event actions, so its existing behavior remains unchanged.
- EWS continues to advertise only calendar viewing. No EWS write request becomes reachable through this decision.
- The desktop must represent partial support accurately. It must not show disabled-looking or inert event controls for unsupported operations.
- The application workflows and provider ports already separate create, update, and delete. This decision does not add another workflow layer.
- The domain already depends on `enumset`; no dependency or persistence migration is needed.
- Synthetic provider and headless UI tests can verify action advertisement and control visibility. They do not replace live Exchange verification for EWS operations.

## Sources

- [ADR 0058: Gate EWS mail actions by operation](0058-per-operation-ews-mail-capabilities.md)
- [Issue #389: Define safe EWS calendar event writes](https://github.com/1fach/einfach-mail/issues/389)
- [Issue #390: Model calendar capabilities by operation](https://github.com/1fach/einfach-mail/issues/390)
- `crates/domain/src/account.rs`
- `crates/application/src/calendar/provider.rs`
- `apps/desktop/src/features/calendar/view/actions.rs`
- `apps/desktop/src/features/calendar/composer/render.rs`
