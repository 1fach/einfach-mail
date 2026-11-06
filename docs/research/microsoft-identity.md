# Microsoft identity research

Research checked: 2026-11-01. Use Microsoft's Entra documentation as the source of truth. Verify tenant policy and sign-in behavior against a test tenant before claiming support.

## Desktop app model

- A desktop app is a public client. Do not embed a client secret.
- Use authorization code with PKCE and open the sign-in page in the system browser. The first account profile comes from Graph `/me`; the provider does not request OIDC scopes or consume an ID token.
- Microsoft identity platform v2 requests must explicitly include `offline_access` to receive refresh tokens. Replace a refresh token when the server returns a new one. The account provider requests `User.Read` and `offline_access` for profile lookup and refresh.
- Microsoft recommends a Microsoft authentication library. Its published MSAL library table does not include Rust.
- Microsoft recommends the system browser when an app cannot use MSAL. Microsoft notes that some native-app capabilities, including proof that the app runs on a managed device, may be unavailable without MSAL.
- Microsoft's desktop configuration guide shows `http://localhost` for system-browser sign-in. Its redirect URI guidance prefers `127.0.0.1` because `localhost` can resolve incorrectly on a misconfigured host. That page says Entra ignores the port only for `localhost`; other redirect hosts must match the registered port. It also says Entra does not support `[::1]` redirect URIs. The portal does not let users add an HTTP `127.0.0.1` redirect directly. Add it through the app registration manifest.
- RFC 8252 requires authorization servers to accept any port for loopback IP redirects and advises clients against `localhost` because name resolution can be misconfigured. Microsoft's port-matching guidance conflicts with this requirement for an ephemeral `127.0.0.1` callback. [ADR 0031](../decisions/0031-registered-loopback-port-for-entra.md) requires a nonzero registered port on `127.0.0.1` to follow Microsoft's published matching rule. Synthetic tests can verify listener behavior, but only a dedicated tenant can establish whether Entra accepts the exact registration.

The product brief targets Microsoft 365 work or school accounts first. The first identity spike must test that account type and record whether Conditional Access, MFA, and managed-device policy work without MSAL or WAM. Personal Microsoft account support remains open.

## Windows broker support

Microsoft documents Web Account Manager (WAM) through MSAL for Windows desktop apps. WAM integrates with Windows accounts and supports broker capabilities used by Conditional Access and FIDO flows. Microsoft has not published a Rust MSAL implementation. If a target tenant requires broker capabilities, compare a narrow native authentication component with the Rust system-browser flow before accepting an implementation.

The app requests delegated Microsoft Graph permissions for the signed-in user. It does not request tenant-wide application permissions. See [Microsoft Graph research](microsoft-graph.md) for the staged mail and calendar scopes.

## Thunderbird implementation reference

The [Thunderbird source reference](https://github.com/thunderbird/thunderbird-desktop/tree/357d2886624ab41e271aa1ba95fae257b1c7a12d) is upstream commit `357d2886624ab41e271aa1ba95fae257b1c7a12d` from 2026-10-31. Relevant files include `mailnews/base/src/OAuth2.sys.mjs`, `OAuth2Module.sys.mjs`, `OAuth2Providers.sys.mjs`, and the matching provider and flow tests.

The code is useful for lifecycle questions: it opens the system browser, starts a loopback-only listener on an OS-assigned port, binds the exact callback URI into the authorization request, checks state, exchanges the one-time code, reuses saved refresh credentials, retries an `invalid_grant` once with interactive sign-in, and removes saved credentials when clearing tokens. Its Linux credential equivalent is not reusable: Thunderbird delegates storage to its own login manager, while this app's Linux boundary is the OS credential service accepted in ADR 0004.

Do not copy Thunderbird's Microsoft provider configuration as the app's security design. The built-in Microsoft issuer sets `useExternalBrowser` but leaves `usePKCE` unset, and the OAuth implementation only sends a PKCE challenge when that flag is enabled. Its Graph scope set is behind the disabled-by-default `mail.graph.enabled` preference and requests broad read/write and send scopes. Its OAuth logs also include the authorization response URL, token request form data, and token response JSON. Keep PKCE, request only the delegated scopes needed for enabled features, and never log callback codes or token requests or responses.

This review is source inspection, not a live Entra test. Thunderbird's ephemeral `127.0.0.1` callback does not establish that Entra accepts a registered fixed port for this app. MFA, Conditional Access, callback matching, consent revocation, refresh, and sign-out still require tenant evidence.

## Wino Mail implementation reference

The [Wino Mail source reference](https://github.com/bkaankose/Wino-Mail/tree/517b9b9e42540ed76c732e926f6e05b91d8c322c) is upstream commit `517b9b9e42540ed76c732e926f6e05b91d8c322c` from 2026-10-31 and is GPL-3.0 licensed. We keep it for source inspection. No Wino code is copied into Einfach Mail.

Wino places shared Microsoft authentication in `Wino.Authentication.OutlookAuthenticator`. It delegates MSAL client construction, token-cache attachment, interactive UI, and local account removal to `IOutlookAuthenticationHost`. The shared authenticator first tries `AcquireTokenSilent`. It requests interactive sign-in through the host only when needed and when a live UI is available. It also keeps short-lived access tokens in a process-wide cache keyed by account, client, and scopes. Per-key semaphores prevent concurrent refresh-token redemption. This concurrency issue is relevant when designing our token-refresh path.

Wino's inspected Windows host uses MSAL's Windows broker (WAM), attaches an MSAL Extensions cache, and supplies a parent window. The snapshot has no Linux desktop host, so it does not describe Linux system-browser sign-in or credential storage. The shared sign-out path removes the local cached MSAL account. It does not show that an Entra consent grant was revoked remotely. Issue #3 tracks these as separate lifecycle outcomes.

After sign-in, Wino calls Graph `/me?$select=mail,userPrincipalName` to resolve the mailbox address when it differs from the sign-in UPN. That lookup uses delegated `User.Read`, the least-privileged permission for Graph `/me`. Wino's scope configuration also requests broad mail read, write, and send permissions, calendar read and write permissions, contacts, and tasks. M2 needs permission to read message bodies, which `Mail.ReadBasic` does not allow, so initial sign-in requests delegated `Mail.Read`. Under [ADR 0015](../decisions/0015-idempotent-message-read-state-update.md), an explicit read-state action denied by Microsoft can lead to a separate permission review requesting `Mail.ReadWrite` in place of `Mail.Read`. That permission also allows creating and deleting mail; the app does not request `Mail.Send`. See [ADR 0013](../decisions/0013-graph-account-identity.md). Wino's source provides implementation context, not tenant evidence for callback registration, MFA, Conditional Access, refresh, consent revocation, or sign-out.

## Rust library review

ADR 0003 selects Huskarl 0.11.10 for Linux authorization code with PKCE, state, loopback callback, exchange, and refresh. Initial sign-in requests delegated `User.Read`, `Mail.Read`, `Calendars.Read`, and `offline_access` through the `organizations` authority; it does not request OIDC scopes or consume ID tokens. After an explicit read-state denial, ADR 0015 adds same-account permission renewal that requests `Mail.ReadWrite` in place of `Mail.Read` before replacing the saved refresh token. Reqwest rejects redirects for token and Graph requests. `webbrowser` 1.2.6 uses its `hardened` feature to launch the HTTPS authorization URL in the system browser. The new consent request and Graph write remain unverified against a dedicated tenant under issue #24.

Axum 0.8.9 was evaluated as a loopback HTTP server candidate, but Huskarl accepts a caller-owned Tokio listener and owns callback parsing and cancellation cleanup. `loopauth` 0.5.1 uses Axum and `reqwest` 0.13, but its API targets CLI token acquisition, and its detached callback task has no caller cancellation handle. Its CI workflow runs on Ubuntu only. `oauthkit` targets CLI use and has no verified Windows build. `oauth-toolkit` 0.4.2 has a blocking, handwritten HTTP callback parser and a failing RustSec audit. OAuth2 and OpenID Connect protocol crates were considered; the selected flow does not need their added ID-token and discovery behavior. The production provider uses the system browser and Linux Secret Service. Synthetic tests do not establish callback registration, default-browser behavior, MFA, Conditional Access, refresh, consent revocation, or sign-out against Entra. See [ADR 0003](../decisions/0003-desktop-authentication.md) for the accepted scope and remaining checks.

## Sources

- [Microsoft authorization code flow](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow)
- [Thunderbird OAuth implementation at the reviewed snapshot](https://github.com/thunderbird/thunderbird-desktop/blob/357d2886624ab41e271aa1ba95fae257b1c7a12d/mailnews/base/src/OAuth2.sys.mjs)
- [Thunderbird Microsoft OAuth provider configuration](https://github.com/thunderbird/thunderbird-desktop/blob/357d2886624ab41e271aa1ba95fae257b1c7a12d/mailnews/base/src/OAuth2Providers.sys.mjs)
- [Wino Mail shared Outlook authenticator at the reviewed snapshot](https://github.com/bkaankose/Wino-Mail/blob/517b9b9e42540ed76c732e926f6e05b91d8c322c/src/Wino.Authentication/OutlookAuthenticator.cs)
- [Wino Mail Windows MSAL host at the reviewed snapshot](https://github.com/bkaankose/Wino-Mail/blob/517b9b9e42540ed76c732e926f6e05b91d8c322c/src/Wino.Platform.Windows/WindowsOutlookAuthenticationHost.cs)
- [Microsoft identity platform scopes and OIDC](https://learn.microsoft.com/en-us/entra/identity-platform/scopes-oidc)
- [Configure desktop apps that call web APIs](https://learn.microsoft.com/en-us/entra/identity-platform/scenario-desktop-app-configuration)
- [Redirect URI best practices and limitations](https://learn.microsoft.com/en-us/entra/identity-platform/reply-url)
- [Add a redirect URI to an app registration](https://learn.microsoft.com/en-us/entra/identity-platform/how-to-add-redirect-uri)
- [RFC 8252, section 7.3: loopback interface redirection](https://www.rfc-editor.org/rfc/rfc8252.html#section-7.3)
- [RFC 8252, section 8.3: loopback redirect considerations](https://www.rfc-editor.org/rfc/rfc8252.html#section-8.3)
- [Microsoft identity platform app types](https://learn.microsoft.com/en-us/entra/identity-platform/v2-app-types)
- [MSAL overview and supported libraries](https://learn.microsoft.com/en-us/entra/identity-platform/msal-overview)
- [Microsoft Azure Identity for Rust](https://docs.rs/azure_identity/1.0.0/azure_identity/)
- [Microsoft guidance for apps that cannot use MSAL](https://learn.microsoft.com/en-us/entra/architecture/authenticate-applications-and-users)
- [Acquire a token by using Windows Account Manager](https://learn.microsoft.com/en-us/entra/identity-platform/scenario-desktop-acquire-token-wam)
- [`oauth2` 5.0.0 documentation](https://docs.rs/oauth2/5.0.0/oauth2/)
- [`openidconnect` 4.0.1 documentation](https://docs.rs/openidconnect/4.0.1/openidconnect/)
- [`webbrowser` 1.2.6 release notes](https://github.com/amodm/webbrowser-rs/releases/tag/v1.2.6)
- [RustSec RUSTSEC-2026-0257 for `webbrowser`](https://rustsec.org/advisories/RUSTSEC-2026-0257.html)
- [`axum` 0.8.9 graceful shutdown](https://docs.rs/axum/0.8.9/axum/serve/struct.Serve.html#method.with_graceful_shutdown)
- [`loopauth` 0.5.1 documentation](https://docs.rs/loopauth/0.5.1/loopauth/)
- [`loopauth` authorization-flow source](https://github.com/joshuaclayton/loopauth/blob/main/src/builder.rs)
- [`loopauth` callback server source](https://github.com/joshuaclayton/loopauth/blob/main/src/server.rs)
- [`loopauth` CI workflow](https://github.com/joshuaclayton/loopauth/blob/main/.github/workflows/ci.yml)
- [`oauthkit` 0.2.4 documentation](https://docs.rs/oauthkit/0.2.4/oauthkit/)
- [`oauth-toolkit` 0.4.2 loopback source](https://github.com/WyattAu/oauth-toolkit/blob/8a9a3dd4f8a2d0811049d887af30c9ba6e1c1794/src/loopback.rs)
- [`oauth-toolkit` 0.4.2 RustSec run](https://github.com/WyattAu/oauth-toolkit/actions/runs/41157846278)
