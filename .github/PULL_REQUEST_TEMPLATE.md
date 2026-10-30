## User outcome

What can a user or maintainer do after this change?

## Acceptance criteria

- [ ] Link the issue or explain why no issue is needed.
- [ ] Update product, architecture, decision, setup, or status docs where facts changed.
- [ ] Review new or changed dependencies in `docs/project/dependency-review.md`.
- [ ] Add or update behavior tests and synthetic provider fixtures where needed.

## Risk and recovery

Describe platform, permission, privacy, persistence, sync, and rollback risks.

## Checks and evidence

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] `cargo test --workspace --locked`
- [ ] Attach UI or test-tenant evidence for behavior that source checks cannot prove.

List the checks actually run. Do not claim checks that did not run.
