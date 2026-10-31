# Microsoft identity research

Research checked: 2026-10-30. Use Microsoft's Entra documentation as the source of truth. Tenant policy and sign-in behavior must be verified against a test tenant before implementation.

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

The dependency review compares `oauth2`, `openidconnect`, `loopauth`, `oauthkit`, and the HTTP adapter needed by the chosen Graph client. The turnkey loopback candidates target CLI apps and still need Windows and Entra-policy proof. No authentication dependency is accepted. See [ADR 0003](../decisions/0003-desktop-authentication.md) for the alternatives and spike criteria.

## Sources

- [Microsoft authorization code flow](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow)
- [Configure desktop apps that call web APIs](https://learn.microsoft.com/en-us/entra/identity-platform/scenario-desktop-app-configuration)
- [Redirect URI best practices and limitations](https://learn.microsoft.com/en-us/entra/identity-platform/reply-url)
- [Microsoft identity platform app types](https://learn.microsoft.com/en-us/entra/identity-platform/v2-app-types)
- [MSAL overview and supported libraries](https://learn.microsoft.com/en-us/entra/identity-platform/msal-overview)
- [Microsoft Azure Identity for Rust](https://docs.rs/azure_identity/1.0.0/azure_identity/)
- [Microsoft guidance for apps that cannot use MSAL](https://learn.microsoft.com/en-us/entra/architecture/authenticate-applications-and-users)
- [Acquire a token by using Windows Account Manager](https://learn.microsoft.com/en-us/entra/identity-platform/scenario-desktop-acquire-token-wam)
