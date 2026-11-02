# ADR 0031: Require a registered loopback port for Entra

- Status: Accepted for Linux implementation; tenant verification remains open
- Date: 2026-11-01
- Owners: Einfach Mail maintainers

## Context

The Linux desktop uses a system browser and a loopback listener for OAuth authorization-code with PKCE. The listener previously defaulted to an OS-assigned port on `127.0.0.1`. Microsoft's redirect guidance says it ignores port differences only for `localhost`; for other hosts, the request port must match the app registration. The Microsoft authorization-code guide recommends `http://localhost` for system-browser native apps, while the redirect guide prefers the `127.0.0.1` IP literal. RFC 8252 §7.3 requires authorization servers to accept any port for loopback IP redirects and recommends the IP literal over `localhost` for security.

The project has no dedicated Microsoft 365 test tenant or public app registration. Documentation establishes Microsoft's published port-matching rule, but source review cannot establish the behavior of the app's specific registration or prove the end-to-end callback.

## Decision

Require the desktop app to use a nonzero `EINFACH_MAIL_ENTRA_CALLBACK_PORT` that exactly matches an `http://127.0.0.1:<port>/callback` redirect registered as an installed client. Keep the callback listener bound to IPv4 loopback only. Reject missing and zero ports before enabling account actions. Keep OS-assigned ports available to synthetic protocol tests; the live Entra proof also requires a registered fixed port.

Do not switch to `localhost` based on port flexibility alone. Reconsider that host only with tenant evidence and a security review against RFC 8252's hostname-resolution guidance.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Registered fixed port on `127.0.0.1` | Matches Microsoft's published Entra rule and keeps the listener pinned to an IP loopback address. | Requires an exact app-manifest entry and fails when another process owns the port. Entra behavior remains unverified for this registration. |
| OS-assigned port on `127.0.0.1` | Follows RFC 8252 and avoids port conflicts. | Conflicts with Microsoft's published rule that ignores ports only for `localhost`; cannot be claimed to work with Entra without tenant evidence. |
| OS-assigned port on `localhost` | Microsoft documents ignoring localhost port differences. | Conflicts with RFC 8252's recommendation against `localhost` because name resolution can be misconfigured. Microsoft also prefers the IP literal to avoid network-interface and firewall misconfiguration. |

## Consequences

- Desktop setup requires a client ID and a nonzero callback port registered in the app manifest. Invalid or missing configuration leaves account actions unavailable.
- Synthetic callback tests still bind port zero and prove the listener, PKCE, state, callback validation, cancellation, and cleanup behavior only.
- Issue #3 remains open for the exact registered callback, port-conflict failure, browser behavior, tenant policies, refresh, consent revocation, and sign-out proof. Do not claim live Entra support without that evidence.
- This decision covers Linux. Re-evaluate callback registration and browser behavior when Windows or macOS support begins.

## Sources

- [Microsoft identity platform authorization-code flow](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow)
- [Microsoft redirect URI guidance](https://learn.microsoft.com/en-us/entra/identity-platform/reply-url)
- [Microsoft app manifest `replyUrlsWithType`](https://learn.microsoft.com/en-us/entra/identity-platform/reference-app-manifest#replyurlswithtype-attribute)
- [RFC 8252 §7.3, loopback interface redirection](https://www.rfc-editor.org/rfc/rfc8252.html#section-7.3)
- [RFC 8252 §8.3, loopback redirect considerations](https://www.rfc-editor.org/rfc/rfc8252.html#section-8.3)
- [ADR 0003: Desktop authentication](0003-desktop-authentication.md)
- [Issue #3: Prove delegated Entra desktop sign-in](https://github.com/1fach/einfach-mail/issues/3)
