# ADR 0054: Discover Linux Exchange EWS endpoints with Autodiscover

- Status: Accepted
- Date: 2026-11-09
- Owners: Einfach Mail maintainers

## Context

Issue [#258](https://github.com/1fach/einfach-mail/issues/258) adds endpoint discovery to the explicit HTTPS Exchange EWS account flow from [ADR 0053](0053-linux-read-only-exchange-ews.md). The discovery request must not receive NTLM credentials or Graph tokens. Its response and every redirect are untrusted. A discovered EWS endpoint must be visible and editable before the user starts the existing authenticated connection.

Linux clients cannot query Exchange's Active Directory Service Connection Points. Exchange documents HTTPS endpoints derived from the mailbox domain, HTTP and XML redirects, and `_autodiscover._tcp` SRV records. Its EWS POX endpoint returns Exchange protocol candidates and EWS URLs. The Microsoft documentation recommends SOAP when its richer settings are needed, while POX remains supported for this endpoint lookup. Thunderbird's `ExchangeAutoDiscover.sys.mjs` confirms the POX request and response shapes. Its authenticated discovery flow is not adopted. Wino Mail's account setup uses Graph and its generic autodiscovery service does not implement EWS Autodiscover, so it is not an EWS protocol reference.

## Decision

Add a native **Discover** action to the Exchange account form. The action receives only the mailbox address. Its copy explains that discovery sends that address to HTTPS Autodiscover hosts and never sends the username, password, or a token. The user can edit the resulting endpoint and must press **Connect** to begin the separate NTLM connection.

Use the Exchange POX Autodiscover request and response with the existing `quick-xml` dependency. Do not add a SOAP Autodiscover model because the product needs only an EWS URL. Parse the address with the existing `addr-spec` library. Reject domain literals. Use `url` to normalize and validate each resulting host and URL.

Try these candidates in order:

1. `https://autodiscover.<mailbox-domain>/autodiscover/autodiscover.xml`.
2. `https://<mailbox-domain>/autodiscover/autodiscover.xml`.
3. HTTPS endpoints from `_autodiscover._tcp.<mailbox-domain>` SRV records, ordered by ascending priority and then descending weight.

Use Hickory DNS `hickory-resolver` 0.26.3 with only its `system-config` and `tokio` features to resolve SRV records through the Linux system resolver. Do not add DNS-over-HTTPS or a resolver that bypasses the configured system DNS path.

Create a separate reqwest client for Autodiscover. Disable automatic redirects and retries. Send only an XML `Content-Type`; never add an `Authorization` header. Apply TLS certificate verification, an eight-second per-request timeout, a thirty-second discovery deadline, a one-megabyte response limit, a bounded candidate count, and a redirect-loop limit. Use HTTPS only. Reject credentials, query strings, fragments, invalid ports, malformed XML, and non-HTTPS URLs at the provider boundary.

An HTTP redirect, POX `RedirectUrl`, any `RedirectAddr`, or an SRV destination not among the two domain-derived candidates requires a separate confirmation before Einfach Mail sends the mailbox address there. The provider stores one bounded, in-memory continuation, and the UI displays the full HTTPS destination URL before the user selects **Continue**. A missing or cancelled confirmation leaves that destination uncontacted. The user can still enter the explicit HTTPS endpoint manually. Final `EwsUrl` or `ASUrl` values are validated as HTTPS URLs, shown in the endpoint field, and are not contacted by discovery; only the later **Connect** action sends NTLM credentials to the selected endpoint.

For a successful POX response, prefer an `EXPR` EWS URL and fall back to `EXCH`. Ignore other protocol types. Reject redirect loops and stop after a small fixed number of redirect steps. If discovery fails, keep the manual endpoint fallback available.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Keep manual endpoint entry only | No new network discovery surface. | Leaves the setup problem in issue #258 unresolved. |
| Use SOAP Autodiscover | Supports a richer set of Exchange settings and Microsoft recommends it where those settings are needed. | The product needs only an EWS URL; this adds another protocol model without a user outcome. |
| Use POX Autodiscover | Supported Exchange protocol that directly returns the EWS URL; Thunderbird provides an independent, read-only reference for the response shape. | The provider must own bounded XML mapping and destination confirmation. |
| Use `srv-rs` for service lookup | Provides an SRV-oriented service client and target-selection policy. | Its execution callback tries destinations as a side effect, while this flow must inspect and sometimes confirm a target before sending the mailbox address. Hickory exposes the DNS records without that coupling. |
| Follow every redirect automatically | Fewer prompts and more automatic discovery. | Can disclose the mailbox address to a host the user has not seen. |
| Reject all redirects | Simple destination boundary. | Breaks common Exchange deployments that publish HTTPS redirects. |

## Consequences

- The account application gains an unauthenticated, provider-owned discovery operation that is separate from account connection and credential persistence.
- Discovery never receives the NTLM username or password. The EWS authenticated transport remains unchanged.
- External discovery destinations require an explicit user action before a request is sent. The manually configured EWS endpoint remains editable and available when discovery does not succeed.
- Hickory DNS 0.26.3 adds a moderate dependency graph but stays below the Rust 1.95 workspace MSRV. Its enabled features use the system resolver and Tokio without enabling DNS-over-HTTPS, DNS-over-TLS, or DNSSEC.
- Synthetic HTTPS fixtures prove request construction, response mapping, redirect handling, and credential non-disclosure. No live Exchange server is available, so keep issue #258 open for live behavior verification.

## Sources

- [Microsoft: Autodiscover service in Exchange Server](https://learn.microsoft.com/en-us/exchange/architecture/client-access/autodiscover)
- [Microsoft: Autodiscover for Exchange](https://learn.microsoft.com/en-us/exchange/client-developer/exchange-web-services/autodiscover-for-exchange)
- [Microsoft: Use Autodiscover to find connection points](https://learn.microsoft.com/en-us/exchange/client-developer/exchange-web-services/how-to-use-autodiscover-to-find-connection-points)
- [Hickory DNS 0.26.3 manifest](https://github.com/hickory-dns/hickory-dns/blob/v0.26.3/Cargo.toml)
- [Hickory resolver 0.26.3 manifest](https://github.com/hickory-dns/hickory-dns/blob/v0.26.3/crates/resolver/Cargo.toml)
- [Hickory resolver 0.26.3 documentation](https://docs.rs/hickory-resolver/0.26.3/hickory_resolver/struct.Resolver.html)
- [`srv-rs` 1.0.1](https://github.com/deshaw/srv-rs/releases)
- [`addr-spec` 0.10.4](https://docs.rs/addr-spec/0.10.4/addr_spec/struct.AddrSpec.html)
- [`email_address` 0.2.9](https://docs.rs/crate/email_address/0.2.9)
- [Thunderbird Exchange Autodiscover reference](https://github.com/thunderbird/thunderbird-desktop/blob/main/mail/components/accountcreation/modules/ExchangeAutoDiscover.sys.mjs)
- [Wino Mail account discovery reference](https://github.com/bkaankose/Wino-Mail/blob/master/src/Wino.Core/Services/AutoDiscoveryService.cs)
