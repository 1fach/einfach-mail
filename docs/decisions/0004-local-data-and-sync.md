# ADR 0004: Keep a local cache and synchronize it with Graph

- Status: Accepted; encryption design open
- Date: 2026-10-30

## Context

The app should show recently loaded mail and calendar data without waiting on each screen for the network. Graph is the remote authority for mailbox and calendar changes. Delta APIs return pages and opaque state links. The app must recover from interruption without losing remote changes.

## Decision

Keep a durable local cache for data the user has chosen to synchronize. Use Graph as the authority for confirmed remote state. Represent local writes as pending operations until Graph confirms them. Advance sync cursors only in the same transaction that commits all changes from the completed delta round.

Before real mailbox content is stored, choose the database engine, schema migration approach, encryption-at-rest method, key recovery behavior, and account-removal deletion behavior. The choice must be recorded in a follow-up ADR.

## Consequences

- The application layer owns synchronization state transitions and retry policy.
- Persistence owns transactions and cursor durability.
- The Graph adapter owns opaque provider cursor values and API pagination.
- A local operation must have an explicit retry and conflict policy before it enters an outbox.
- Authentication tokens remain separate from cached mail and event data.
- Cache encryption and account deletion are release gates, not optional cleanup.
