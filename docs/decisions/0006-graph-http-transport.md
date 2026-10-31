# ADR 0006: Use reqwest for Microsoft Graph transport

- Status: Proposed; Linux synthetic proof passes, cross-platform CI and any required live Graph evidence pending
- Date: 2026-10-30
- Owners: Project maintainer

## Context

The provider must support Graph v1.0 mail and calendar calls, follow opaque pagination and delta links, and honor Graph throttling. Microsoft does not publish an official Rust Graph SDK.

The current SDK candidates do not meet all project constraints. `io-msgraph` 0.6.0 added calendar and event calls, including calendar-view delta, but its error type drops response headers. That prevents the adapter from reading Graph's `Retry-After` value. Its source also logs request URLs at `trace` level. Its published README still says calendar support is planned. `graph-rs-sdk` 3.0.1 has broad API and paging support, but its latest stable release is dated 2025-04-20. It depends on `reqwest` 0.12 and Tokio. Its optional interactive authentication feature adds Wry and Tao. The full pagination and retry-header path has not been proven.

The desktop uses GPUI Kit and `smol`. `reqwest` uses Tokio, so keep its runtime on a private provider worker. Connect the worker to the caller with `async-channel`. A blocking `ureq` worker is simpler, but each request occupies a thread. `isahc` avoids Tokio and depends on libcurl. Its upstream describes the project as passively maintained. The proof tests the `reqwest` worker before we add it to the application.

## Decision

Use `reqwest`, `serde`, and `serde_json` in the planned `crates/providers/microsoft-graph/` adapter. Keep the candidate dependencies in the proof package's `[dev-dependencies]` until the proof and cross-platform checks pass.

Set `reqwest`'s default features to off. Enable only Rustls, HTTP/2, and system proxy support. Parse Graph JSON with `serde_json` instead of enabling `reqwest`'s JSON feature. Reuse one client. Disable redirects and automatic retries so the adapter controls Graph retry behavior.

Keep Graph requests, API-to-domain mapping, pagination, error mapping, and throttling in the adapter. Do not write a general HTTP client, JSON parser, or OAuth protocol.

The application-facing provider port will expose provider-independent values and errors. It will not expose `reqwest`, Tokio, Graph JSON, or response-header types. The provider will obtain credentials through the approved identity boundary. Views will not receive tokens.

The adapter must preserve response headers so it can honor Graph's `Retry-After` value. Retry only operations whose semantics permit another request. Do not automatically retry a write until the operation's duplicate-request behavior is proven. If Graph omits `Retry-After`, use the documented exponential-backoff behavior.

Treat `@odata.nextLink` and `@odata.deltaLink` as opaque provider values. Before attaching a bearer token, verify that each link uses HTTPS and the configured Graph authority. Disable redirects so a response cannot redirect an authorized request to another host. Request immutable IDs for message and event operations, including delta rounds. Mark authorization headers sensitive and keep tokens, response bodies, and full Graph URLs out of logs.

Use the smallest `reqwest` feature set that supports the chosen TLS backend, HTTP/2, and supported system proxy behavior. Do not enable experimental HTTP/3. Keep the Tokio runtime private to the provider adapter. Accept this ADR and add the dependencies only after the spike below passes. If the runtime bridge fails, compare an executor-agnostic client such as `isahc` and update this ADR.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| `reqwest` plus the Einfach Mail Graph adapter | Maintained async HTTP client, direct access to status and headers, Rustls support, and a small feature set. Custom code stays limited to the Graph operations Einfach Mail needs. | Requires a private Tokio runtime alongside GPUI's `smol`. The adapter owns Graph operation construction and the Graph-specific retry policy. Rustls versions need a current lockfile audit. |
| `graph-rs-sdk` 3.0.1 | Broad Graph v1 and beta APIs, typed operations, and paging helpers. A direct `send` returns a `reqwest::Response`. | No stable release since 2025-04-20. It depends on `reqwest` 0.12 and Tokio. Its pagination retry hook, maintenance fit, and locked dependency security need proof. The `interactive-auth` feature adds Wry and Tao and must stay disabled. |
| `io-msgraph` 0.6.0 | Recent releases, an I/O-free Graph core, Rustls support, and mail and calendar operations. Its MSRV is below the workspace MSRV. | The Graph error type drops `Retry-After`, and request URLs appear in trace logs. The published README conflicts with the changelog about calendar support. Using it would require changes or a workaround that bypasses the library boundary. |
| `isahc` 2.0.2 or `ureq` 3.5.0 | `isahc` offers runtime-agnostic async I/O. `ureq` is a pure-Rust blocking client. Both expose ordinary HTTP responses. | `isahc` uses libcurl and its project marks itself passively maintained. `ureq` uses blocking I/O and would need a separate worker boundary. Neither supplies the Graph operations or Graph throttling policy. |

## Consequences

- The Graph provider owns HTTP transport, response parsing, Graph API types, next-link validation, and Graph throttling. It maps valid results into domain values before returning them to application workflows.
- The GPUI shell and application crate do not depend on Tokio or `reqwest` types.
- This is a proposed direction, not an accepted production dependency. The test-only `tools/graph-transport-proof/` package keeps every candidate under `[dev-dependencies]`.
- The proof keeps Tokio and `reqwest::Client` on a private worker. A test confirms that a `smol` timer advances while a mock request is pending. Dropping the response receiver cancels that request. Shutdown waits for the runtime to drop and joins the worker off the `smol` executor.
- The package has no application API. Tokio and HTTP types do not appear in the desktop or application crate interfaces.
- Synthetic fixtures cover folder pagination, message and calendar-view delta rounds, malformed responses, Graph errors, `Retry-After` seconds and dates, fallback delay, redirects, and unauthorized responses. They also verify cancellation, shutdown, immutable IDs, and opaque-link validation.
- The client does not retry 401 or 403 responses, does not follow redirects, rejects a next link outside the configured Graph authority, and emits no logs containing tokens, response bodies, or full URLs.
- Linux tests and the RustSec audit pass. RustSec found no vulnerable or yanked crates; eight unmaintained transitive platform and rendering crates remain documented in the dependency review. The lockfile selects Rustls 0.23.46. Before accepting the ADR, verify Windows and macOS builds and tests in CI.
- Use a test tenant for Graph behavior that synthetic fixtures cannot establish. Do not use personal mailbox data.

## Sources

- [Microsoft Graph SDK overview](https://learn.microsoft.com/en-us/graph/sdks/sdks-overview)
- [Microsoft Graph throttling guidance](https://learn.microsoft.com/en-us/graph/throttling)
- [Microsoft Graph immutable Outlook identifiers](https://learn.microsoft.com/en-us/graph/outlook-immutable-id)
- [Microsoft Graph calendar-view delta](https://learn.microsoft.com/en-us/graph/delta-query-events?tabs=http)
- [`reqwest` 0.13.5 documentation and manifest](https://docs.rs/crate/reqwest/0.13.5)
- [`reqwest` 0.13.5 manifest and feature list](https://docs.rs/crate/reqwest/0.13.5/source/Cargo.toml)
- [`httpmock` 0.9.0 documentation and dependency list](https://docs.rs/crate/httpmock/0.9.0)
- [`wiremock` 0.6.5 documentation and manifest](https://docs.rs/crate/wiremock/0.6.5)
- [`async-channel` 2.5.0 documentation and manifest](https://docs.rs/crate/async-channel/2.5.0)
- [`httpdate` 1.0.3 documentation](https://docs.rs/httpdate/1.0.3/httpdate/)
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
