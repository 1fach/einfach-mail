# ADR 0062: Tie the message-body future to its borrowed inputs

- Status: Accepted
- Date: 2026-11-12
- Owners: Einfach Mail maintainers

## Context

`MailProvider::fetch_message_body` takes borrowed `self`, folder, and message ID arguments. Its `impl Future + Send + '_` return bound names only the receiver's lifetime. The future cannot borrow the other arguments under this trait contract. Implementations that defer work in an `async move` block clone them first.

Issue [#361](https://github.com/1fach/einfach-mail/issues/361) tested a named shared lifetime on this one method. The account router and persistence fixture no longer need several clones. The Graph and EWS providers still need owned values at their worker boundary, so their message IDs remain cloned. The spike did not inspect all 61 port methods, and their async and ownership boundaries differ.

## Decision

Keep one named lifetime shared by `self`, `folder`, and `message_id` on `MailProvider::fetch_message_body`, and bound its returned future by that lifetime. Do not apply this signature mechanically to the other application port methods. Consider each method separately when its future can borrow its inputs across awaits and that removes clones without crossing an ownership boundary.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Keep the change limited to `fetch_message_body` | Retains the demonstrated clone removal in the router and fixture while preserving worker ownership. | Other methods may continue to clone values that could safely be borrowed. |
| Apply the signature to every port method | Could remove more clones in forwarding implementations. | The spike provides no evidence that all methods can borrow safely; worker and storage boundaries may still require ownership, making a blanket API change noisy. |
| Revert the lifetime change | Avoids changing the application trait signature. | Restores clones already shown to be unnecessary in the router and fixture. |

Choose the targeted change. The evidence does not support a port-wide rollout yet.

## Consequences

- The account router forwards borrowed folder and message identifiers directly.
- Graph keeps a config `Arc` and owned account and message IDs for its worker. EWS keeps an owned message ID for its worker and clones the account ID because session lookup takes ownership.
- The persistence test fixture borrows the account identifier and synchronization gates; it still clones the owned body result for its return value.
- Future lifetime changes require method-specific evidence and workspace compilation, including the `AccountSessionFuture` and `MailSession` boxing paths.

## Sources

- [Issue #361: Spike: let port futures borrow arguments instead of cloning](https://github.com/1fach/einfach-mail/issues/361)
- [Rust Reference: Lifetime elision](https://doc.rust-lang.org/reference/lifetime-elision.html)
- `crates/application/src/mail/provider.rs`
- `crates/providers/account-router/src/lib.rs`
- `crates/providers/microsoft-graph/src/provider/mail.rs`
- `crates/providers/exchange-ews/src/provider/mail.rs`
- `crates/persistence/src/test_support/provider.rs`
