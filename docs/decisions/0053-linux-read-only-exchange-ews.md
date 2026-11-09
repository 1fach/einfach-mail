# ADR 0053: Add Linux read-only Exchange Server mail through EWS

- Status: Accepted
- Date: 2026-11-09
- Owners: Einfach Mail maintainers

## Context

Einfach Mail currently connects to Exchange Online through delegated Microsoft Graph access. Users with Exchange Server installed on premises need a separate account path. Microsoft documents NTLM for on-premises EWS and recommends HTTPS. Exchange Extended Protection can require TLS channel binding. The current account workflow also assumes that every account exposes Graph mail writes and a calendar.

Issue [#257](https://github.com/1fach/einfach-mail/issues/257) defines the first EWS scope: Linux, an explicit HTTPS endpoint, NTLMv2, read-only mail, and no calendar. [Issue #258](https://github.com/1fach/einfach-mail/issues/258) tracks Autodiscover as a follow-up. No Exchange test server or sanitized live response capture is available, so synthetic tests cannot establish live compatibility.

## Decision

Keep Exchange Online on the Microsoft Graph provider. Add a separate `einfach-mail-exchange-ews` provider for read-only on-premises mail and a provider composition boundary that selects the single account type saved by the application.

Use provider-neutral account capabilities to describe which mail and calendar actions an account supports. Mark EWS mail as read-only and its calendar as unavailable. The desktop hides unsupported actions and views. Provider methods also reject unsupported writes so application callers cannot bypass the UI gate.

Use the maintained `ews` crate for EWS SOAP types and XML serialization. Guard its malformed-envelope panic path at the response boundary. Use the existing `reqwest` HTTP client with redirects disabled, HTTP/1.1 only, platform TLS verification, bounded responses, and serialized NTLM handshakes. Use Devolutions `sspi` directly in NTLM mode for NTLMv2; its secret credential types zeroize the password when dropped. Parse the TLS-validated peer certificate with `x509-parser` and derive the RFC 5929 `tls-server-end-point` bytes with `sha2`, following only the certificate signature algorithms with a single defined hash. Keep the HTTP handshake connection-bound and verify that each challenge leg uses the same TCP connection before sending the next token.

Accept only a user-entered HTTPS EWS endpoint without embedded credentials. Do not follow redirects or send credentials to a new URL. Defer Autodiscover to issue #258. Defer EWS writes to the follow-up scope in ADR 0058, calendar operations, Exchange Online EWS, ADFS OAuth, Windows, and macOS.

Store the password only in Linux Secret Service. Store the endpoint and mailbox identity in application configuration without a password. Keep EWS message sync state in the existing encrypted SQLCipher cache and reuse the application's 90-day initial mail cutoff and page-commit rules.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Use EWS for Exchange Online | One provider could cover both Exchange products. | Microsoft directs Exchange Online clients to Graph, and EWS retirement applies to Exchange Online. It would duplicate the Graph provider and use a retired Exchange Online API. |
| Keep Graph as the only provider | No new account or protocol boundary. | On-premises Exchange accounts cannot connect through Graph in the supported desktop flow. |
| Write SOAP requests and NTLMv2 by hand | Full control over request shapes and authentication. | Duplicates maintained XML and authentication code, expands the security review, and risks protocol parsing and channel-binding errors. |
| Use `winauth` for NTLM | It implements NTLMv2 and channel bindings with a small API. | The reviewed 0.0.5 release has old cryptographic dependencies and does not zeroize its password. Its API also leaves HTTP connection handling to this application. |
| Use `negotiate-auth` for NTLM | It is a focused sans-I/O HTTP Negotiate library with channel-binding support and its companion `mapi-client` reports live Exchange Server SE testing. The maintainer has approved raising the workspace MSRV if a better library requires it. | Version 0.4.0 is a new 0.x crate, and its identity owns a cloneable password in a plain `String` that cannot be zeroized through its public API. Reject for credential handling, independent of MSRV. |
| Use Devolutions `sspi` with the `tls-cbt` helper | SSPI is maintained, cross-platform, implements NTLMv2, and stores passwords in zeroizing secret types. `tls-cbt` is a small RFC 5929 helper with reported fuzzing and Windows reference results. | `tls-cbt` has a very small maintenance history and maps unrecognized signature algorithms to SHA-256. That can produce an incorrect binding for certificates whose signature hash is not in its supported set. Use the more mature `x509-parser` and select the binding digest from its parsed signature algorithm instead. |
| Use `ews`, `sspi`, `x509-parser`, `sha2`, and `reqwest` | Maintained Rust libraries cover EWS SOAP types, NTLMv2, certificate parsing, cryptographic digests, and HTTP. X.509 parsing stays in a mature parser; the adapter implements only RFC 5929's digest selection rule and rejects unsupported signatures. | The application still owns bounded request sequencing, connection identity, response-to-domain mapping, account selection, and the supported read-only feature set. `ews` is MPL-2.0 and follows Firefox's MSRV policy rather than declaring a fixed MSRV. The selected `sspi` 0.23.1 requires Rust 1.93, below the current workspace 1.95; no MSRV increase is needed for this selection. |

## Consequences

- The application adds an Exchange account form and keeps Graph sign-in available when its public-client configuration is present.
- EWS uses the existing account cache, message sanitizer, and native GPUI renderer.
- The account model and views gate the initial read-only mail scope and unavailable calendar features. EWS writes remain unsupported until the operation-specific capability in ADR 0058 is implemented; calendar writes remain unsupported.
- EWS authentication is enabled only for a validated HTTPS endpoint. A failed certificate check, redirect, malformed challenge, or connection change ends the request without sending another NTLM token.
- SSPI 0.23.1 pins `rsa 0.10.0-rc.18`, which triggers RUSTSEC-2023-0071. This application invokes only SSPI's NTLM provider and has no RSA private key or private-key operation in its EWS path, so `.cargo/audit.toml` ignores only that advisory. Keep the rest of the RustSec audit active and remove the exception when SSPI adopts a fixed stable RSA release; [issue #259](https://github.com/1fach/einfach-mail/issues/259) tracks the update.
- The first implementation has no live-server evidence. Keep issue #257 open until a real Exchange Server or sanitized recording verifies authentication, hierarchy paging, message paging, and body retrieval.
- The workspace remains at Rust 1.95 if the selected locked dependency graph compiles there. The maintainer has approved an MSRV increase when a better library requires one and its security, maintenance, platform, license, and dependency criteria pass.

## Sources

- [Microsoft: Authentication and EWS in Exchange](https://learn.microsoft.com/en-us/exchange/client-developer/exchange-web-services/authentication-and-ews-in-exchange)
- [Microsoft: Extended Protection in Exchange Server](https://learn.microsoft.com/en-us/exchange/plan-and-deploy/post-installation-tasks/security-best-practices/exchange-extended-protection?view=exchserver-2019)
- [Microsoft: Exchange Server on-premises Modern Authentication](https://learn.microsoft.com/en-us/exchange/plan-and-deploy/post-installation-tasks/enable-modern-auth-in-exchange-server-on-premises)
- [Thunderbird `ews-rs`](https://github.com/thunderbird/ews-rs)
- [Devolutions `sspi-rs` 0.23.1](https://docs.rs/crate/sspi/0.23.1)
- [`x509-parser` 0.18.1](https://docs.rs/crate/x509-parser/0.18.1)
- [`negotiate-auth` 0.4.0](https://docs.rs/crate/negotiate-auth/0.4.0)
- [`tls-cbt` 0.1.0](https://github.com/icedracon/tls-cbt)
- [`mapi-client-rs`](https://github.com/allodia-eu/mapi-client-rs)
