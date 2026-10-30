# ADR 0003: Choose a desktop authentication implementation

- Status: Proposed; authentication spike required
- Date: 2026-10-30

## Context

The desktop app is a public client. It cannot keep a client secret confidential. Microsoft recommends authorization code with PKCE and OpenID Connect for desktop apps. Microsoft documents `http://localhost` as the redirect URI for system-browser sign-in.

Microsoft's desktop library list does not include a Rust MSAL library. Microsoft recommends using a supported authentication library rather than constructing raw HTTP flows. The project must verify how a Rust desktop app can meet those requirements, including Windows single sign-on and tenant policies.

## Proposed direction

Use system-browser authorization code with PKCE. Keep token operations in Rust. Store refresh credentials using an operating-system credential service. Do not expose tokens to UI views or store them in the local application database.

Do not implement account sign-in until an authentication spike passes the checks below and a new ADR accepts the implementation.

## Spike acceptance criteria

- Sign in and cancel sign-in through the system browser without embedding the Microsoft sign-in page in the app.
- Verify the redirect callback and PKCE verifier handling on Windows.
- Test work or school accounts, the selected personal-account policy, MFA, and Conditional Access with a test tenant.
- Refresh tokens without prompting when allowed by tenant policy. Handle revoked consent and sign-out.
- Store and remove credentials through the selected operating-system credential service.
- Prove that tokens never enter UI state, application logs, SQLite, crash reports, or test artifacts.
- Document any Microsoft-supported library or platform broker used, and identify remaining support gaps.

## Options to evaluate

- Rust OAuth/OIDC library using system browser and loopback redirect.
- A platform broker for Windows, if it can work safely with the Rust application and the target account types.
- A small native authentication component that uses an official Microsoft library, if the Rust option cannot meet Conditional Access and support requirements.

## Sources

See the [Microsoft Graph research note](../research/microsoft-graph.md) for Microsoft identity documentation.
