# ADR 0004: Keep a local cache and synchronize it with Graph

- Status: Accepted; encryption detailed in ADR 0005
- Date: 2026-10-30

## Context

The app should show recently loaded mail and calendar data without waiting on each screen for the network. Graph is the remote authority for mailbox and calendar changes. Delta APIs return pages and opaque state links. The app must recover from interruption without losing remote changes.

## Decision

Keep a durable local cache for data the user has chosen to synchronize. Use Graph as the authority for confirmed remote state. Represent local writes as pending operations until Graph confirms them.

For each sync scope, keep the last completed `@odata.deltaLink` and an optional in-progress `@odata.nextLink` checkpoint. Commit each page's data changes and its next-link checkpoint in one transaction. Keep the completed delta link unchanged while pages remain. On the final page, commit its data changes and the new delta link, and clear the checkpoint in the same transaction. After interruption, resume from the checkpoint when present; otherwise start from the completed delta link or the initial query. Reapplying a page before its checkpoint commits must be safe.

Scope both links to the exact account, collection, and query settings that produced them. Store them with the encrypted cache. The application workflow must allow only one active sync round per cursor scope. Do not report a scope as synchronized until its final delta link is durably committed.

Before real mailbox content is stored, choose the database engine, schema migration approach, encryption-at-rest method, key recovery behavior, and account-removal deletion behavior. The choice must be recorded in a follow-up ADR.

## Consequences

- The application layer owns synchronization state transitions, retry policy, and one active round per cursor scope.
- Persistence owns atomic page/checkpoint commits and final delta-link durability.
- The Graph adapter owns opaque provider cursor values and API pagination.
- A local operation must have an explicit retry and conflict policy before it enters an outbox.
- Authentication tokens remain separate from cached mail and event data.
- Cache encryption and account deletion are release gates, not optional cleanup.
