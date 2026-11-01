# ADR 0030: Order account cancellation against credential commits

- Status: Accepted
- Date: 2026-11-01
- Owners: Einfach Mail maintainers

## Context

The account view can request cancellation while OAuth or permission renewal is in progress. Dropping the GPUI task at that point can lose the provider result even when the synchronous credential store has already started writing a refresh token. Showing a cancelled state in that case can misreport a committed account as disconnected. Permission renewal also must not replace the active identity or overwrite the existing credential when the user cancels or signs in as another account.

Issue #45 requires accurate cancellation behavior for first connection and permission renewal. This decision is limited to the Linux M1 account flow. Permission renewal requests only the existing delegated `User.Read` and `offline_access` scopes. It does not add mail, calendar, or sending permissions.

## Decision

The application owns an account-operation cancellation handle with atomic `Active`, `Cancelled`, and `CredentialCommitStarted` phases. The user cancellation request and the provider's credential-commit start race through compare-and-exchange on the same phase:

- If cancellation changes `Active` to `Cancelled` first, the provider stops authorization and does not save a new credential.
- If the provider changes `Active` to `CredentialCommitStarted` first, cancellation does not interrupt the synchronous credential write. The account view retains the task and shows the final operation result.

The provider must call `begin_credential_commit` immediately before saving a refresh credential. Permission renewal must verify the Graph profile has the active account ID before reaching that boundary. The workflow keeps its single-account operation reservation until the provider result is applied or the workflow future is dropped.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Atomically order cancellation against credential commit | Makes the race explicit, keeps cancellation before persistence retryable, and reports a commit that already won accurately | Adds one application-owned handle that providers must honor; it cannot make a synchronous Secret Service write interruptible |
| Drop the view task and report cancellation immediately | Minimal view state | Can hide a credential write already in progress and report a false cancelled state |
| Treat every cancellation request as advisory and always finish authorization | Simple provider flow | Continues browser and network work after user cancellation, including when credential commit has not begun |
| Reimplement cancellation signaling with a custom future and waker | Avoids an external type | Adds general-purpose async synchronization code without improving the commit-order guarantee; `tokio-util` is already used by the provider |

## Consequences

- The view shows `Finishing sign-in` or `Finishing permission review` after cancellation while a provider operation resolves.
- A cancellation that wins before commit preserves the prior credential and leaves the operation retryable.
- A commit that wins first resolves to its actual profile or error; permission renewal cannot silently switch accounts.
- Synthetic tests cover both race orders, retry, profile mismatch, and restoring a committed credential through a new provider instance. These tests do not prove live Entra or every Secret Service implementation.
- Closing the account view can still drop its task. A synchronous credential write already executing on the provider worker runs to completion; the next launch can restore the saved credential.
- No live Microsoft 365 request or real mailbox data is required to verify this local state transition.

## Sources

- [Issue #45: Keep account cancellation state accurate across credential commits](https://github.com/1fach/einfach-mail/issues/45)
- [`tokio-util` `CancellationToken` API](https://docs.rs/tokio-util/0.7.20/tokio_util/sync/struct.CancellationToken.html)
- [Tokio `tokio-util` 0.7.20 changelog](https://github.com/tokio-rs/tokio/blob/tokio-util-0.7.20/tokio-util/CHANGELOG.md)
