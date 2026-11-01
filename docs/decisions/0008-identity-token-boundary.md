# ADR 0008: Keep identity tokens inside the Microsoft provider

- Status: Accepted
- Date: 2026-10-30
- Owners: Project maintainer

## Context

Einfach Mail's first provider is Microsoft Graph for Exchange Online. The app needs both an account connection flow and Graph calls. OAuth access and refresh tokens are secrets. They must not enter GPUI views, application workflow results, the local mail database, logs, or message content.

The workspace separates the native desktop app, application workflows, domain concepts, and infrastructure adapters. [ADR 0003](0003-desktop-authentication.md) selects the Linux authorization-code flow and its libraries. ADR 0006 accepts Reqwest for the Linux Graph provider.

## Decision

Keep Microsoft identity and Graph HTTP code in `crates/providers/microsoft-graph/`. Its private `identity` and `graph` modules implement Linux account connection and profile lookup. Do not add a separate identity crate until a second provider or a platform broker needs an independent build boundary.

The application owns provider-neutral account, mail, and calendar workflows and ports. The Microsoft provider implements those ports. Application contracts carry domain values and application errors only. They do not carry OAuth tokens, OAuth protocol types, or Graph response types.

The provider's private identity module owns browser sign-in, PKCE, token exchange and refresh, and credential-store access. It gives short-lived access tokens only to the provider's Graph HTTP client. It stores refresh credentials through the selected operating-system credential service. The identity implementation must use maintained protocol and platform libraries. It may contain Einfach Mail-specific orchestration, but it must not reimplement OAuth or a general-purpose callback server.

The desktop composition root wires the application workflows to one Microsoft provider instance. GPUI views receive account state, domain data, and user-safe errors. They never receive tokens or call identity, Graph, or credential-store code.

Linux implementation and synthetic provider fixtures are in place. Do not claim tenant support or close issue #3 until callback registration, default-browser behavior, MFA, Conditional Access, refresh, consent revocation, and sign-out pass in a dedicated test tenant. Defer Windows-specific broker and credential-store checks until M5, and macOS checks until M6, under [ADR 0010](0010-linux-first-development.md).

## Usage sketch

```text
account = connect_account.run()
page = list_messages.run(account.id, folder.id)
```

The application calls a provider-neutral port and receives domain values. The provider acquires or refreshes tokens inside its identity module before it sends an authorized Graph request.

## Ownership and module map

| Owner | Responsibility | Data that may cross its application boundary |
| --- | --- | --- |
| `apps/desktop` | GPUI lifecycle, native views, keyboard actions, and application composition | User actions, domain results, loading state, and user-safe errors |
| `crates/application` | Account, mail, and calendar workflows and provider ports | Domain values and application errors |
| `crates/domain` | Provider-independent account, mail, calendar, and event invariants | Domain values only |
| `crates/providers/microsoft-graph/src/identity/` | Entra browser flow, PKCE, token lifecycle, and operating-system credential access | Nothing crosses to the application layer as a token |
| `crates/providers/microsoft-graph/src/graph/` | Graph HTTP, response parsing, pagination, throttling, and domain mapping | Domain values and mapped provider errors |

The provider's identity and Graph modules may share private token state. Only the identity module owns that state. The persistence adapter stores account metadata and synced data, never OAuth credentials.

## Flow

1. A GPUI view asks an application workflow to connect an account or load data.
2. The workflow calls its application port. It does not know the OAuth flow or Graph wire format.
3. For account connection, the provider's identity module opens the system browser, runs the selected authorization flow, and stores the refresh credential in the operating-system credential service.
4. The provider uses a private access token to request the Graph profile or requested data. It parses Graph responses at that boundary and maps them into domain values.
5. The workflow returns domain results and safe errors to GPUI. No token crosses the application port.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Private identity module inside the Microsoft Graph provider | Keeps tokens within one provider boundary. The application sees only domain values. One crate owns the Entra and Graph lifecycle that shares the credentials. | Identity is initially coupled to the Microsoft provider. Split it only when a second real consumer or a platform-specific build boundary justifies that cost. |
| Separate Entra identity crate consumed by the Graph provider | Gives OAuth, broker code, and credential handling an independent build boundary. A Windows broker could be isolated from other targets. | Adds a crate and a cross-crate token interface for one consumer. The current proof has not shown that the broker needs a separate build. |
| Application workflow acquires and passes tokens to the Graph provider | Makes authentication and Graph requests explicit to the workflow. | Sends secret values through application types and expands the places that could log or expose credentials. The application would also need OAuth and provider-specific scope concepts. |
| GPUI views manage sign-in and Graph calls | The view can start browser work directly. | Couples UI state to Entra, Graph, credential storage, and token handling. It violates the accepted dependency direction and token boundary. |

## Consequences

- The application exposes provider-neutral account, mail, and calendar ports. It does not expose an access-token port to views or workflows.
- The Microsoft Graph provider owns internal identity state and Graph request authorization. Its access token type stays private to that adapter.
- The system browser is the sign-in surface. The app does not embed the Microsoft sign-in page in a WebView.
- ADR 0003 selects Huskarl, `webbrowser`, Reqwest, and `keyring` for Linux account connection. Recheck maintenance, platforms, licensing, MSRV, features, security history, and dependency cost before changing those choices.
- If a test tenant requires WAM or another native broker, add a narrow Windows implementation behind the provider's private identity boundary. Review its FFI and Windows window lifecycle in a separate ADR before implementation.
- Split identity into another crate only when a second provider or a broker with distinct platform requirements needs that boundary.
- Synthetic tests verify that dropping the sign-in task releases its loopback listener. Repeat the browser and provider checks against Entra before claiming live support.

## Sources

- [RFC 8252: OAuth 2.0 for Native Apps](https://datatracker.ietf.org/doc/html/rfc8252)
- [Microsoft authorization code flow](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow)
- [Microsoft MSAL overview and supported libraries](https://learn.microsoft.com/en-us/entra/identity-platform/msal-overview)
- [Microsoft WAM desktop flow](https://learn.microsoft.com/en-us/entra/identity-platform/scenario-desktop-acquire-token-wam)
- [System architecture](../architecture/system.md)
- [ADR 0003: Desktop authentication implementation](0003-desktop-authentication.md)
- [ADR 0006: Graph HTTP transport](0006-graph-http-transport.md)
