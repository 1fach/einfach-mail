# ADR 0003: Choose a desktop authentication implementation

- Status: Proposed; authentication spike required
- Date: 2026-10-31

## Context

The desktop app is a public client. It cannot keep a client secret confidential. Microsoft recommends authorization code with PKCE and OpenID Connect through the system browser. The desktop configuration guide shows `http://localhost` for system-browser sign-in. Microsoft's redirect guidance prefers the loopback IP literal `127.0.0.1` because `localhost` can resolve incorrectly. Entra ignores the port for loopback redirect matching, but adding an HTTP IP-literal redirect requires editing the app registration manifest. Verify that registration path with a test tenant before choosing the callback host.

Microsoft's published MSAL library list does not include Rust. The official `azure_identity` Rust crate documents developer-tool, Azure-hosted, and service-principal credentials, but not an interactive desktop public-client flow. Microsoft recommends MSAL where available and the system browser when MSAL cannot be used. Microsoft also notes that some native-app capabilities, such as proving that the app runs on a managed device, may not be available without MSAL. Windows WAM broker support is documented through MSAL and includes Conditional Access, FIDO, and Windows account integration.

Rust libraries cover parts of the flow. `oauth2` and `openidconnect` provide protocol implementations, but do not open the browser or receive a loopback callback. `loopauth` combines those desktop steps, but its documented scope is CLI applications and its Windows and Entra-policy fit remain unproven. The dependency review compares these shapes. No authentication library or broker is accepted. Use maintained libraries for OAuth/OIDC protocol behavior. Write only Einfach Mail-specific orchestration unless the review proves that no suitable library covers a required step.

## Proposed direction

Use a system browser and authorization code with PKCE. Prefer a loopback callback bound only to `127.0.0.1`; if Entra registration or a supported tenant requires another host, record the reason and verify the alternative. Keep protocol and token operations in Rust. Store refresh credentials using an operating-system credential service. Do not expose tokens to UI views or store them in the local application database. Do not embed the sign-in page in a WebView.

ADR 0008 accepts the provider ownership boundary for these credentials. Keep the OAuth library, callback mechanism, and credential-store implementation open until the authentication spike passes.

Do not implement account sign-in until an authentication spike passes the checks below and a new ADR accepts the implementation.

## Candidate designs

| Design | Benefit | Cost and open risk |
| --- | --- | --- |
| `oauth2` plus `openidconnect`, with maintained browser and loopback libraries | Uses established, strongly typed protocol crates. Each crate owns a clear protocol boundary. | Neither protocol crate owns browser launch or callback capture. The callback lifecycle and cancellation path still need a suitable maintained library. Their bundled HTTP adapter uses `reqwest` 0.12. Using the proposed `reqwest` 0.13 requires the pre-release `oauth2-reqwest` adapter or a narrow HTTP adapter. `openidconnect` does not validate `azp` and depends on `rsa`, which has an unpatched RustSec Marvin timing advisory. |
| `huskarl` with its loopback feature and a separate browser launcher | Provides authorization-code with PKCE, state handling, OIDC nonce and ID-token issuer/audience checks, JWKS refresh, and a callback server. The separate `huskarl-reqwest` adapter targets `reqwest` 0.13. | Version 0.11.10 requires Rust 1.92, is MIT OR Apache-2.0, and is pre-1.0. Its default crypto backend depends on `rsa` 0.10.0-rc.19 and exposes RSA private-key signing code. RustSec has no patched RSA release and lists rc.18 as affected, so rc.19 needs a direct status check. Huskarl's ID-token validator intentionally ignores mismatching `azp` values. Its callback waits on a caller-owned Tokio listener, so the app must prove that cancellation drops the listener and waiting task. The tagged CI workflow has Ubuntu runners only. Disabling the default backend allows a custom verifier platform. `jsonwebtoken` with `aws_lc_rs` is a possible verifier, but AWS-LC adds a C/C++ build and no adapter to Huskarl has been tested. Huskarl does not launch the system browser, so it still needs a separate library. |
| `loopauth` | Provides system-browser launch, loopback callback, PKCE, token exchange, refresh, and OIDC support in one library. It already uses `reqwest` 0.13. | It describes itself as a CLI library. Its public flow has a callback timeout but no caller cancellation method. Its current docs.rs build lists Linux x86-64, and its repository has no verified Windows or Entra policy proof. The repository's README and the published crate documentation report different versions. |
| `oauthkit` | Provides a loopback authorization-code flow with PKCE and state validation, OIDC discovery, a callback timeout, and pluggable credential storage. | The upstream describes it as a CLI SDK and labels the project beta. Its repository has little release history, and this review has not established Windows builds, Entra behavior, managed-device Conditional Access, or GPUI cancellation and shutdown behavior. |
| Microsoft MSAL with WAM through a native component | Follows Microsoft's documented Windows broker path and may support tenant policies unavailable to a browser-only flow. | Microsoft does not publish an MSAL implementation for Rust. A native component introduces a language and lifecycle boundary and may not cover non-Windows targets. |

No authentication implementation is selected. Keep the Rust library candidate review in [the dependency review](../project/dependency-review.md), and keep Microsoft's platform constraints in [the identity research note](../research/microsoft-identity.md). ADR 0008 records the accepted token-ownership boundary without choosing an OAuth implementation.

## Spike acceptance criteria

- Sign in and cancel sign-in through the system browser without embedding the Microsoft sign-in page in the app.
- Verify the Entra app registration, loopback host and ephemeral port, redirect callback, PKCE verifier, state, and OIDC nonce handling on Windows.
- Prove that cancellation closes the callback listener and releases any waiting task.
- Test work or school accounts, the selected personal-account policy, MFA, and Conditional Access with a test tenant. Include a managed-device policy case and record any limitation without MSAL or WAM.
- Refresh tokens without prompting when allowed by tenant policy. Handle revoked consent and sign-out.
- Store and remove credentials through the selected operating-system credential service.
- Prove that tokens never enter UI state, application logs, SQLite, crash reports, or test artifacts.
- Compare `oauth2` plus `openidconnect`, `huskarl`, `loopauth`, `oauthkit`, and `oauth-toolkit` for the required flow. Document current maintenance, supported targets, license, feature cost, cancellation behavior, and security evidence.
- If a candidate needs a different JWS verifier, use a maintained cryptography library. Do not implement signature algorithms in Einfach Mail.
- Review the exact selected dependency graph with RustSec. Disable HTTP redirects for token and discovery requests.
- Document any Microsoft-supported library or platform broker used, and identify remaining support gaps.

## Sources

See the [Microsoft identity research note](../research/microsoft-identity.md) for Microsoft identity documentation.
