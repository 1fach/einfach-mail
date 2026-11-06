# ADR 0006: Use reqwest for Microsoft Graph transport

- Status: Accepted for Linux provider implementation
- Date: 2026-10-31
- Owners: Project maintainer

## Context

The provider must support Graph v1.0 mail calls, follow opaque pagination and delta links, and honor Graph throttling. Microsoft does not publish an official Rust Graph SDK.

The current SDK candidates do not meet all project constraints. `io-msgraph` 0.6.0 added calendar and event calls, including calendar-view delta, but its error type drops response headers. That prevents the adapter from reading Graph's `Retry-After` value. Its source also logs request URLs at `trace` level. Its published README still says calendar support is planned. `graph-rs-sdk` 3.0.1 has broad API and paging support, but its latest stable release is dated 2025-04-20. It depends on `reqwest` 0.12 and Tokio. Its optional interactive authentication feature adds Wry and Tao. The production Linux mail transport and retry-header path now have synthetic fixture coverage; live Graph behavior remains unverified.

The desktop currently uses GPUI Kit. The active Linux dependency graph uses `smol`; asynchronous `reqwest` requires Tokio. Any Tokio runtime must stay inside the Graph provider and must not leak into GPUI or application interfaces.

## Decision

Use `reqwest` and `serde` for HTTP and JSON in `crates/providers/microsoft-graph/`. Keep Einfach Mail's Graph endpoint calls, API-to-domain mapping, pagination, error mapping, and throttling policy in that adapter. Do not write a general HTTP client, JSON parser, or OAuth protocol.

Send `Content-Type: application/json` on message-delta GET requests, including requests that follow an opaque continuation link, as required by the Graph v1.0 message-delta contract.

The application-facing provider port exposes provider-independent values and errors. It does not expose `reqwest`, Tokio, Graph JSON, or response-header types. The provider obtains credentials through the approved identity boundary. Views do not receive tokens.

The adapter must preserve response headers so it can honor Graph's `Retry-After` value. Retry only operations whose semantics permit another request. Do not automatically retry a write until the operation's duplicate-request behavior is proven. If Graph omits `Retry-After`, use the documented exponential-backoff behavior.

Treat `@odata.nextLink` and `@odata.deltaLink` as opaque provider values. Before attaching a bearer token, verify that each link uses HTTPS and the configured Graph authority. Disable redirects so a response cannot redirect an authorized request to another host. Mark authorization headers sensitive and keep tokens, response bodies, and full Graph URLs out of logs.

Use the smallest `reqwest` feature set that supports the chosen TLS backend, JSON, HTTP/2, and supported system proxy behavior. Do not enable experimental HTTP/3. Keep the Tokio runtime private to the provider adapter. The production Linux implementation and synthetic transport proof satisfy the local runtime and provider-boundary checks. Keep live Graph behavior unverified until a test tenant is available.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| `reqwest` plus the Einfach Mail Graph adapter | Maintained async HTTP client, direct access to status and headers, Rustls support, and a small feature set. Custom code stays limited to the Graph operations Einfach Mail needs. | Requires a private Tokio runtime alongside GPUI's `smol`. The adapter owns Graph operation construction and the Graph-specific retry policy. Rustls versions need a current lockfile audit. |
| `graph-rs-sdk` 3.0.1 | Broad Graph v1 and beta APIs, typed operations, and paging helpers. A direct `send` returns a `reqwest::Response`. | No stable release since 2025-04-20. It depends on `reqwest` 0.12 and Tokio. Its pagination retry hook, maintenance fit, and locked dependency security need proof. The `interactive-auth` feature adds Wry and Tao and must stay disabled. |
| `io-msgraph` 0.6.0 | Recent releases, an I/O-free Graph core, Rustls support, and mail and calendar operations. Its MSRV is below the workspace MSRV. | The Graph error type drops `Retry-After`, and request URLs appear in trace logs. The published README conflicts with the changelog about calendar support. Using it would require changes or a workaround that bypasses the library boundary. |
| `isahc` 2.0.2 or `ureq` 3.5.0 | `isahc` offers runtime-agnostic async I/O. `ureq` is a pure-Rust blocking client. Both expose ordinary HTTP responses. | `isahc` uses libcurl and its project marks itself passively maintained. `ureq` uses blocking I/O and would need a separate worker boundary. Neither supplies the Graph operations or Graph throttling policy. |

For runtime ownership, a per-operation worker and a provider-owned worker were compared. A per-operation worker would rebuild the client and runtime for each folder or message request and would not provide shared backpressure or a provider-level shutdown point. The provider-owned worker matches the transport proof and makes request cancellation, concurrency limits, proxy behavior, and shutdown one owned lifecycle. Tokio MPSC is sufficient because the worker is the sole command receiver; the test-only proof may continue using `async-channel` for its cross-executor harness. No production channel dependency is added.

## Consequences

- The Graph provider owns HTTP transport, response parsing, Graph API types, next-link validation, and Graph throttling. It maps valid results into domain values before returning them to application workflows.
- The GPUI shell and application crate do not depend on Tokio or `reqwest` types.
- The local Linux production provider and proof use read-only Graph operations. Synthetic fixtures cover folder and message responses, message delta paging and removals, opaque-link validation, immutable IDs, redirects, HTTP 429 `Retry-After`, cancellation, shutdown, proxy routing, response limits, and provider error redaction.
- The proof keeps Tokio and HTTP types private to the Graph adapter. It uses no logging dependency and stores no credentials or real mailbox content.
- Issue #7's local transport criteria are complete. Do not claim live pagination, throttling, consent, or mailbox behavior without test-tenant evidence. Do not use personal mailbox data.
- Run RustSec against the production lockfile before any real Graph request. Defer Windows and macOS validation to their support milestones.

## Sources

- [Microsoft Graph SDK overview](https://learn.microsoft.com/en-us/graph/sdks/sdks-overview)
- [Microsoft Graph throttling guidance](https://learn.microsoft.com/en-us/graph/throttling)
- [Microsoft Graph message delta v1.0](https://learn.microsoft.com/en-us/graph/api/message-delta?view=graph-rest-1.0)
- [`reqwest` 0.13.5 documentation and manifest](https://docs.rs/crate/reqwest/0.13.5)
- [`reqwest` retry behavior](https://docs.rs/reqwest/0.13.5/reqwest/retry/)
- [`reqwest-retry-after` 0.2.1 documentation](https://docs.rs/reqwest-retry-after/0.2.1/reqwest_retry_after/)
- [`graph-rs-sdk` 3.0.1 documentation](https://docs.rs/crate/graph-rs-sdk/3.0.1)
- [`io-msgraph` 0.6.0 changelog](https://docs.rs/crate/io-msgraph/0.6.0/source/CHANGELOG.md)
- [`io-msgraph` Graph error handling](https://raw.githubusercontent.com/pimalaya/io-msgraph/v0.6.0/src/v1/send.rs)
- [GPUI Kit async guide](../../.agents/skills/gpui-kit/references/gpui/async.md)
- [RustSec RUSTSEC-2026-0346 for rustls](https://rustsec.org/advisories/RUSTSEC-2026-0346)
- [RustSec RUSTSEC-2026-0347 for rustls](https://rustsec.org/advisories/RUSTSEC-2026-0347)
- [RustSec RUSTSEC-2026-0348 for rustls](https://rustsec.org/advisories/RUSTSEC-2026-0348)
- [RustSec RUSTSEC-2026-0349 for rustls](https://rustsec.org/advisories/RUSTSEC-2026-0349)
