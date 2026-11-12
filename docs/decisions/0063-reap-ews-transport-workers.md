# ADR 0063: Reap EWS transport workers without blocking Drop

- Status: Accepted
- Date: 2026-11-12
- Owners: Einfach Mail maintainers

## Context

`EwsTransport` owns a Tokio command channel, a shutdown sender, and the worker thread's `JoinHandle`. Its `Drop` sends shutdown and joins that thread. The worker owns the `SecretString` password and the `reqwest::Client` in `EwsTransportState`.

The worker checks shutdown before taking a queued command and while a request is in flight. The HTTP client also sets a 10-second connect timeout and a 35-second request timeout. Even with those bounds, synchronous `join` blocks whichever thread drops the last transport owner.

An `EwsSession` lives in the provider's shared `RwLock<Option<Arc<EwsSession>>>`. Connect and restore construct sessions on provider workers. Mail and calendar operations clone the session and run through the provider runtime, which gives each operation its own Tokio thread. The GPUI account view owns the provider composition root, so it can also release an idle session during view teardown. A dropped operation retains its session until the provider worker cancels and releases it.

Issue [#383](https://github.com/1fach/einfach-mail/issues/383) requires shutdown to distinguish queued work from a request that may have reached Exchange. It also requires the worker to retain a clear owner until its secret state is dropped.

## Decision

Create one process-wide worker reaper with `std::sync::mpsc` and `std::thread`. Initialize it before starting the first EWS transport worker. If the reaper thread cannot start, return `ServiceUnavailable` before creating a transport.

When `EwsTransport` drops, send the shutdown signal and enqueue its worker `JoinHandle`. The reaper owns and joins each handle. The transport's destructor does not wait for normal worker shutdown. If the reaper has stopped and rejects a handle, join that worker synchronously rather than detach it.

The worker continues to use Tokio's shutdown selection. A queued read returns `ServiceUnavailable`; a queued update returns `NotDispatched`. An in-flight read returns `ServiceUnavailable`; an in-flight update returns `OutcomeUnknown`. Never retry an update whose dispatch may have started.

Keep the password inside `EwsTransportState`. The worker drops that state when shutdown exits `run_worker`. The reaper keeps the `JoinHandle` until the worker thread finishes, so no transport worker is silently detached while it owns credentials.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Join synchronously in `Drop` | The caller knows the worker exited before `Drop` returns. | Blocks the dropping thread while the worker observes shutdown and destroys its state. |
| Drop the `JoinHandle` after sending shutdown | `Drop` returns without waiting or starting another thread. | Detaches the worker and leaves no owner to confirm it released its credential. |
| Use Tokio `spawn_blocking` to join | Reuses Tokio's blocking pool when a runtime is active. | `Drop` can run during GPUI teardown without a Tokio runtime, so this depends on ambient runtime state and needs another fallback. |
| Require every caller to await `close` | Makes successful cleanup observable at explicit call sites. | `EwsSession` is shared through `Arc`; replacement and cancellation can release the last owner outside a single close path. `Drop` still needs a safe fallback. |
| Use one standard-library reaper thread | Keeps `Drop` non-blocking in the normal case and retains every worker handle until join. | Adds one process-lifetime thread and a small queue. If the reaper fails after initialization, the sender's returned handle uses synchronous fallback join. |

Use the standard-library reaper. Tokio has no runtime-independent join operation for the owned OS thread, and this lifecycle needs no new dependency.

## Consequences

- Dropping an EWS transport signals shutdown and transfers its worker handle to the reaper. The caller can continue before the worker exits.
- Each worker remains responsible for its `SecretString` until `run_worker` exits. The reaper retains the handle and joins the worker.
- Queued updates rejected before worker execution are reported as not dispatched. Updates interrupted after worker execution starts remain outcome-unknown.
- A failed reaper startup prevents new EWS transports. A reaper channel failure falls back to synchronous join instead of detaching the credential-owning worker.
- The reaper waits for worker cancellation, with the existing 35-second HTTP timeout as the network fallback bound. Synthetic tests cover idle transport drop, queued reads and updates, in-flight reads, and in-flight updates. They do not verify a live Exchange Server.

## Sources

- [Issue #383: Define EWS transport worker shutdown lifecycle](https://github.com/1fach/einfach-mail/issues/383)
- `crates/providers/exchange-ews/src/transport.rs`
- `crates/providers/exchange-ews/src/transport/worker.rs`
- `crates/providers/exchange-ews/src/provider/session.rs`
- `crates/providers/runtime/src/lib.rs`
- [Tokio runtime shutdown](https://docs.rs/tokio/latest/tokio/runtime/struct.Runtime.html#shutdown)
