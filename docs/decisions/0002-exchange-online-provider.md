# ADR 0002: Use Microsoft Graph v1.0 for Exchange Online

- Status: Accepted
- Date: 2026-10-30

## Context

The first supported provider target is Microsoft 365 and Exchange Online. The app needs mail and calendar APIs that work for a signed-in user and support incremental synchronization.

Microsoft recommends Graph for Exchange Online. Microsoft Graph does not support Exchange Server installed on premises. Microsoft's current EWS retirement timeline starts phased global disablement in October 2026 and fully disables EWS in April 2027.

## Decision

Use Microsoft Graph v1.0 with delegated permissions for the signed-in user. Do not use EWS for Exchange Online. Keep provider request and response types inside the Graph adapter. Convert them to provider-independent domain values before application code uses them.

Use Graph delta queries for message folders and bounded calendar views where the v1.0 API supports them. Persist opaque next and delta links as provider cursor values. Do not depend on beta endpoints for release behavior.

## Alternatives

| Option | Decision |
| --- | --- |
| Exchange Web Services for Exchange Online | Rejected. It is legacy and being retired. |
| Microsoft Graph v1.0 | Accepted. It is Microsoft's recommended API for Exchange Online and has delegated mail and calendar permissions. |
| IMAP, SMTP, and CalDAV for Microsoft 365 | Deferred. These do not replace Graph calendar and account behavior as a single integration. |
| Graph for Exchange Server on premises | Unsupported. Microsoft says Graph does not support on-premises Exchange. |

## Consequences

- The first release does not support on-premises Exchange Server.
- Use the least delegated scopes for enabled features. Do not request application permissions.
- Authentication, throttling, pagination, delta cursor handling, recurrence, and provider limits need explicit tests.
- A future on-premises provider needs its own ADR and security review.

## Sources

See the [Microsoft Graph research note](../research/microsoft-graph.md) for source links and the checked date.
