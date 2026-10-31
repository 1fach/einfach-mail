# Microsoft identity research

Research checked: 2026-10-31. Use Microsoft's Entra documentation as the source of truth. Tenant policy and sign-in behavior must be verified against a test tenant before implementation.

## Desktop app model

- A desktop app is a public client. Do not embed a client secret.
- Use authorization code with PKCE and OpenID Connect. Open the sign-in page in the system browser.
- Microsoft recommends a Microsoft authentication library. Its published MSAL library table does not include Rust.
- Microsoft recommends the system browser when an app cannot use MSAL. Microsoft notes that some native-app capabilities, including proof that the app runs on a managed device, may be unavailable without MSAL.
- Microsoft's desktop configuration guide shows `http://localhost` for system-browser sign-in. Its redirect URI guidance prefers `127.0.0.1` because `localhost` can resolve incorrectly on a misconfigured host. Entra ignores the port for loopback redirect matching. The portal does not let users add an HTTP IP-literal redirect directly. Add it through the app registration manifest and verify the setup with a test tenant.

The product brief targets Microsoft 365 work or school accounts first. The first identity spike must test that account type and record whether Conditional Access, MFA, and managed-device policy work without MSAL or WAM. Personal Microsoft account support remains open.

## Windows broker support

Microsoft documents Web Account Manager (WAM) through MSAL for Windows desktop apps. WAM integrates with Windows accounts and supports broker capabilities used by Conditional Access and FIDO flows. Microsoft has not published a Rust MSAL implementation. If a target tenant requires broker capabilities, compare a narrow native authentication component with the Rust system-browser flow before accepting an implementation.

The app requests delegated Microsoft Graph permissions for the signed-in user. It does not request tenant-wide application permissions. See [Microsoft Graph research](microsoft-graph.md) for the staged mail and calendar scopes.

## Rust library review

Use `oauth2` for OAuth authorization-code with PKCE and token exchange. Use `openidconnect` for discovery and ID-token checks. The `oauth2` crate warns that its token HTTP client must not follow redirects. Keep that setting disabled to prevent SSRF.

Use `webbrowser` 1.2.5 or later to open the HTTPS authorization URL in the system browser. Enable its `hardened` feature, which rejects other URL schemes. RustSec patched its Unix browser-argument injection issue in 1.2.2.

Axum 0.8.9 is the leading loopback HTTP server candidate. It provides graceful shutdown, but adds Tokio, Hyper, and Tower to an app whose GPUI Kit dependency graph currently uses `smol`. Prove the Tokio boundary and cancellation behavior before adding it. `loopauth` 0.5.1 already uses Axum and `reqwest` 0.13, but its API targets CLI token acquisition, and its detached callback task has no caller cancellation handle. Its CI workflow runs on Ubuntu only. `oauthkit` targets CLI use and has no verified Windows build. `oauth-toolkit` 0.4.2 has a blocking, handwritten HTTP callback parser and a failing RustSec audit. No authentication dependency is accepted until the Windows and Microsoft test-tenant proof in issue #3 passes. See [ADR 0003](../decisions/0003-desktop-authentication.md) for the alternatives and spike criteria.

## Sources

- [Microsoft authorization code flow](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow)
- [Configure desktop apps that call web APIs](https://learn.microsoft.com/en-us/entra/identity-platform/scenario-desktop-app-configuration)
- [Redirect URI best practices and limitations](https://learn.microsoft.com/en-us/entra/identity-platform/reply-url)
- [Microsoft identity platform app types](https://learn.microsoft.com/en-us/entra/identity-platform/v2-app-types)
- [MSAL overview and supported libraries](https://learn.microsoft.com/en-us/entra/identity-platform/msal-overview)
- [Microsoft Azure Identity for Rust](https://docs.rs/azure_identity/1.0.0/azure_identity/)
- [Microsoft guidance for apps that cannot use MSAL](https://learn.microsoft.com/en-us/entra/architecture/authenticate-applications-and-users)
- [Acquire a token by using Windows Account Manager](https://learn.microsoft.com/en-us/entra/identity-platform/scenario-desktop-acquire-token-wam)
- [`oauth2` 5.0.0 documentation](https://docs.rs/oauth2/5.0.0/oauth2/)
- [`openidconnect` 4.0.1 documentation](https://docs.rs/openidconnect/4.0.1/openidconnect/)
- [`webbrowser` 1.2.5 release notes](https://github.com/amodm/webbrowser-rs/releases/tag/v1.2.5)
- [RustSec RUSTSEC-2026-0257 for `webbrowser`](https://rustsec.org/advisories/RUSTSEC-2026-0257.html)
- [`axum` 0.8.9 graceful shutdown](https://docs.rs/axum/0.8.9/axum/serve/struct.Serve.html#method.with_graceful_shutdown)
- [`loopauth` 0.5.1 documentation](https://docs.rs/loopauth/0.5.1/loopauth/)
- [`loopauth` authorization-flow source](https://github.com/joshuaclayton/loopauth/blob/main/src/builder.rs)
- [`loopauth` callback server source](https://github.com/joshuaclayton/loopauth/blob/main/src/server.rs)
- [`loopauth` CI workflow](https://github.com/joshuaclayton/loopauth/blob/main/.github/workflows/ci.yml)
- [`oauthkit` 0.2.4 documentation](https://docs.rs/oauthkit/0.2.4/oauthkit/)
- [`oauth-toolkit` 0.4.2 loopback source](https://github.com/WyattAu/oauth-toolkit/blob/8a9a3dd4f8a2d0811049d887af30c9ba6e1c1794/src/loopback.rs)
- [`oauth-toolkit` 0.4.2 RustSec run](https://github.com/WyattAu/oauth-toolkit/actions/runs/41157846278)
