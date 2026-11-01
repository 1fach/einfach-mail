# ADR 0003: Choose a desktop authentication implementation

- Status: Accepted for Linux implementation; live Entra validation remains open
- Date: 2026-10-31

## Context

Linux is the current development target. Windows and macOS support are deferred under [ADR 0010](0010-linux-first-development.md). Select and prove an authentication path for Linux first. Revisit WAM and other Windows-only requirements when Windows support begins.

The desktop app is a public client. It cannot keep a client secret confidential. Use the authorization-code flow with PKCE through the system browser. Microsoft recommends this flow for native apps. Its desktop authorization-code guidance uses `http://localhost`, while its redirect guidance prefers `127.0.0.1` because `localhost` can resolve incorrectly. Entra ignores the port only for `localhost`; other redirect hosts must match the registered port. RFC 8252 requires authorization servers to accept any port on loopback IP redirects. [ADR 0031](0031-registered-loopback-port-for-entra.md) requires a fixed registered port on `127.0.0.1` to follow Microsoft's published matching rule. The RFC conflict remains open until a dedicated tenant verifies the app registration. Never bind the listener outside loopback.

Microsoft's published MSAL library list does not include Rust. The official `azure_identity` Rust crate documents developer-tool, Azure-hosted, and service-principal credentials, but not an interactive desktop public-client flow. Microsoft recommends MSAL where available and the system browser when MSAL cannot be used. Microsoft also notes that some native-app capabilities, such as proving that the app runs on a managed device, may not be available without MSAL. Windows WAM broker support is documented through MSAL and includes Conditional Access, FIDO, and Windows account integration.

Microsoft does not publish an MSAL library for Rust. Wino uses MSAL and WAM in its Windows host, while Thunderbird uses its own OAuth flow and loopback listener. The Rust dependency review found no maintained library that provides both a native system-browser flow and Entra's multi-tenant OIDC issuer rules without unresolved security or lifecycle gaps. The first account connection needs a Graph access token and obtains the mailbox profile from Graph `/me` under [ADR 0013](0013-graph-account-identity.md). Use an OAuth library for the code flow and keep only product-specific orchestration in the provider.

## Decision

Use `huskarl` 0.11.10 with default features disabled and its authorization-code loopback feature. It owns PKCE, state, code exchange, callback parsing, and refresh grants. Use `huskarl-reqwest` 0.9.3 with redirects disabled for token requests. Use `webbrowser` 1.2.6 or later with its `hardened` feature to open the authorization URL. Use `reqwest` 0.13 for Graph requests and `keyring` 4.2 with its v1 Linux Secret Service backend for refresh credentials. Keep synchronous credential-store calls off the GPUI thread.

Use the Microsoft `organizations` authority because the first release supports work or school accounts. Request `User.Read` and `offline_access` for the initial account-connection workflow. Do not request a client secret or application permission. The application calls Graph `/me` for the profile; see ADR 0013. Do not request OIDC ID-token scopes for this flow.

Store only the refresh token in Secret Service. Replace it after each successful refresh when Entra returns a new token. Keep access tokens in provider memory and send them only to Microsoft Graph. Do not return tokens through application ports or expose them to GPUI views, logs, the local database, crash reports, or message content. Delete the local refresh credential on sign-out. This does not revoke the tenant's consent grant.

The synthetic Linux proof is sufficient to begin implementation. Issue #3 remains open for live Entra checks of callback registration, default-browser launch, MFA, Conditional Access, refresh, revoked consent, and sign-out. Do not claim live tenant support or close issue #3 without that evidence. The implementation may use synthetic provider fixtures while no test tenant or public app registration is available.

## Candidate designs

| Design | Benefit | Cost and open risk |
| --- | --- | --- |
| `oauth2` plus `openidconnect`, with maintained browser and loopback libraries | Uses established, strongly typed protocol crates. Each crate owns a clear protocol boundary. | Neither protocol crate owns browser launch or callback capture. The callback lifecycle and cancellation path still need a suitable maintained library. Their bundled HTTP adapter uses `reqwest` 0.12. Using `reqwest` 0.13 requires the pre-release `oauth2-reqwest` adapter or a narrow HTTP adapter. `openidconnect` does not validate `azp` and depends on `rsa`, which has an unpatched RustSec Marvin timing advisory. |
| `huskarl` with its loopback feature and a separate browser launcher | Provides authorization-code with PKCE, state handling, OIDC nonce and ID-token issuer/audience checks, JWKS refresh, and a callback server. The separate `huskarl-reqwest` adapter targets `reqwest` 0.13. | Version 0.11.10 requires Rust 1.92, is MIT OR Apache-2.0, and is pre-1.0. Its default crypto backend depends on `rsa` 0.10.0-rc.19. RustSec has no patched RSA release. Huskarl ignores mismatching `azp` values. The test-only proof disables the default verifier and supplies a custom RS256 platform using `jsonwebtoken` with `aws_lc_rs`; it passes a synthetic flow on Linux and proves callback cancellation releases the listener. AWS-LC adds a C/C++ build. The flow proof does not test Entra keys, Windows, or an actual system browser. Huskarl does not launch the system browser, so a separate library remains necessary. |
| `loopauth` | Provides system-browser launch, loopback callback, PKCE, token exchange, refresh, and OIDC support in one library. It already uses `reqwest` 0.13. | It describes itself as a CLI library. Its public flow has a callback timeout but no caller cancellation method. Its current docs.rs build lists Linux x86-64, and its repository has no verified Windows or Entra policy proof. The repository's README and the published crate documentation report different versions. |
| `oauthkit` | Provides a loopback authorization-code flow with PKCE and state validation, OIDC discovery, a callback timeout, and pluggable credential storage. | The upstream describes it as a CLI SDK and labels the project beta. Its repository has little release history, and this review has not established Windows builds, Entra behavior, managed-device Conditional Access, or GPUI cancellation and shutdown behavior. |
| Microsoft MSAL with WAM through a native component | Follows Microsoft's documented Windows broker path and may support tenant policies unavailable to a browser-only flow. | Microsoft does not publish an MSAL implementation for Rust. A native component introduces a language and lifecycle boundary and may not cover non-Windows targets. |

The selection is limited to Linux implementation. The registered callback and live tenant behavior remain unverified. Keep the broader dependency alternatives and evidence in [the dependency review](../project/dependency-review.md) and [the Microsoft identity research note](../research/microsoft-identity.md). ADR 0008 owns the token boundary.

## Synthetic test evidence

The Microsoft provider has synthetic tests for the PKCE callback and token exchange, Graph profile mapping, refresh-token rotation, consent denial, callback cancellation and cleanup, and sensitive authorization headers. Headless GPUI tests cover the connected identity view and unavailable setup state. These tests do not open a desktop browser or contact Entra.

Issue #3 remains open for default-browser behavior, callback registration, MFA, Conditional Access, refresh, consent revocation, and sign-out against a dedicated test tenant. No tenant evidence is available for this candidate.

## Evidence gates

[Issue #5](https://github.com/1fach/einfach-mail/issues/5) tracks the source-based implementation and its synthetic Linux evidence. Keep [issue #3](https://github.com/1fach/einfach-mail/issues/3) open until a dedicated test tenant and public app registration are available. Record the exact callback behavior, supported tenant policies, credential lifecycle, and remaining platform gaps there before claiming Entra support. Defer Windows and macOS broker checks under ADR 0010.

## Sources

See the [Microsoft identity research note](../research/microsoft-identity.md) for Microsoft identity documentation.

- [Microsoft identity platform scopes and OIDC](https://learn.microsoft.com/en-us/entra/identity-platform/scopes-oidc)
