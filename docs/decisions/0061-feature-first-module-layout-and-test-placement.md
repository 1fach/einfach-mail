# ADR 0061: Use a feature-first module layout and colocated tests

- Status: Accepted
- Date: 2026-11-10
- Owners: Einfach Mail maintainers

## Context

The workspace has 91 tracked Rust source files. The desktop root contains feature views beside bootstrap and platform code. The application crate mixes account, mail, and calendar modules. Several provider, persistence, and desktop files combine multiple behaviors, and test modules use inconsistent locations. The inventory and item declarations are recorded in [the path map on issue #282](https://github.com/1fach/einfach-mail/issues/282).

Issue #282 defines a behavior-preserving refactor. It allows no feature, public API, dependency, schema, migration, or wire-format changes. The layout must keep each existing crate boundary and make ownership visible from paths.

The desktop also has a lifecycle constraint. The root workspace owns one `AccountSession` shared by Mail and Calendar. [ADR 0043](0043-per-account-mail-and-calendar-session.md) defines that session boundary. [ADR 0059](0059-native-mail-and-calendar-workspace.md) permits a replacement for `AccountView` to own the session. [ADR 0060](0060-one-active-account-first-release.md) limits the first release to one active account.

## Decision

Use `account`, `mail`, and `calendar` as the feature names across crates. Add `shared` only when a module serves more than one feature. Add `platform` only for operating-system behavior. Do not add a crate unless it owns a real rule or boundary.

Organize each crate by feature, then by role:

| Crate | Target layout |
| --- | --- |
| `crates/domain` | `account/`, `mail/`, `calendar/`, and `shared/` for identifiers used across features |
| `crates/application` | `account/`, `mail/`, and `calendar/`; keep each workflow separate from its provider and cache ports; put shared reservations and recovery in `shared/` |
| `crates/persistence` | `account/`, `mail/`, and `calendar/` adapters; put shared SQL schema and store code in `shared/`; put Linux paths, file locks, and key storage under `platform/linux/` |
| `crates/providers/microsoft-graph` and `crates/providers/exchange-ews` | `config`, `auth` or `identity`, `transport`, `wire`, `provider/{account,mail,calendar}`, and `error`; omit roles a provider does not use |
| `crates/providers/account-router` | Keep provider selection in `lib.rs`; preserve the current crate-root exports |
| `crates/providers/runtime` | Keep shared provider worker and cancellation behavior in the crate root while it remains a small, single-purpose module |
| `apps/desktop` | Keep `main.rs` as bootstrap; use `app/` for the shell, `platform/` for Linux integration, `components/` for reusable GPUI widgets without feature workflows, `features/{account,mail,calendar}/` for product views, and `test_support/` for shared UI builders |

Use the `foo.rs` and `foo/` convention for every Rust module in `apps/` and `crates/`. Declare module `foo` in `foo.rs`, and put its child modules in `foo/`. Do not use `mod.rs` or `#[path]`, including in tests. This keeps the compiler's module tree aligned with the directory tree and lets file paths show ownership directly.

Do not use `shared/` as a desktop layout role. Put reusable cross-feature GPUI widgets in `components/`, feature-specific helpers inside their feature, and test-only helpers shared by tests in `test_support/`.

Keep `crates/providers/account-router` in one `lib.rs`. The crate has one responsibility: route account, mail, and calendar operations to the selected provider. Splitting those implementations by feature would divide one routing boundary without giving the new modules separate ownership.

In the desktop feature folders, keep view state, actions, rendering, user-facing copy, and tests in focused modules. Keep the shared `AccountSession` owned by the root workspace. Keep account connection and permission review in the account feature. Keep message-body rendering in the mail feature and pass it only sanitized content from the application layer.

Keep private unit tests in a sibling `tests.rs` or `tests/` directory next to the module they test. Keep tests inline only when the small test set remains easy to read with its implementation; move larger test modules beside it. Split a test file above about 800 lines by behavior. Split a production module above about 1,000 lines by owned behavior. Record a reason in the PR when a file stays above either limit. Put public API tests in the crate's `tests/` directory. Give each crate one test-only `test_support` module when multiple test modules share builders.

Private module paths and imports may change during restructuring. Keep every crate-root `lib.rs` public re-export and every item's visibility unchanged. If a sibling module needs access to a private item, add the narrowest `pub(super)` re-export. Never widen an item to `pub(crate)` or `pub`. Keep test names and the total `cargo test --workspace -- --list` count unchanged. Do not change implementation logic while moving items.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Feature first, then role | Keeps related account, mail, and calendar code together while separating workflows, adapters, UI actions, and tests. | Requires moving files and updating private module paths across the workspace. |
| Role first, such as `models/`, `workflows/`, and `tests/` | Makes each technical role easy to search. | Scatters one feature across the crate and gives related modules different top-level paths. |
| Keep the current flat roots and split only the largest files | Limits the number of moves. | Leaves the same feature ownership and test placement questions for each later change. |

Choose feature first, then role. It matches the finalized convention in issue #282 and keeps the code paths aligned across domain, application, persistence, and providers.

## Consequences

- Public crate APIs remain stable even when private source paths change. Domain and application root re-exports remain the supported import paths.
- Persistence moves keep SQL text, migration order, table names, and transaction boundaries unchanged.
- Provider moves keep request shapes, authentication ordering, response mapping, retry policy, and unknown-write recovery unchanged.
- Desktop moves keep the workspace session owner, view lifecycle, focus behavior, selected identities, and keyboard actions unchanged.
- Large test files split by behavior. Cross-layer workflow tests stay in persistence tests when they exercise the provider, application, and cache together.
- Move whole files before splitting their contents when practical. Review source changes separately from path-only moves and compare the test count before and after each sub-issue.
- Complete domain, application, persistence, and provider sub-issues before moving desktop views. This limits conflicts with the active M10 workspace work.

## Sources

- [Issue #282: Restructure the Rust workspace into a feature-first module layout](https://github.com/1fach/einfach-mail/issues/282)
- [Issue #336: Make the Rust module tree match the feature-first directory layout](https://github.com/1fach/einfach-mail/issues/336)
- [Issue #337: Amend ADR 0061 with module declaration rules](https://github.com/1fach/einfach-mail/issues/337)
- [Issue #283: Record the feature-first module layout and test placement ADR](https://github.com/1fach/einfach-mail/issues/283)
- [ADR 0043: Use one per-account mail and calendar session](0043-per-account-mail-and-calendar-session.md)
- [ADR 0059: Use a shared, adaptive mail and calendar workspace](0059-native-mail-and-calendar-workspace.md)
- [ADR 0060: Support one active account in the first release](0060-one-active-account-first-release.md)
- `python3 scripts/untangle-code-map.py --top 100 --commits 100 --min-cochange 3`
