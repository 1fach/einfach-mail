# Einfach Mail

Einfach Mail is a Rust desktop mail and calendar client with Microsoft 365 integration. The native UI uses GPUI Kit. Account sign-in, mail, calendar, and synchronization are not implemented yet.

The first provider target is Exchange Online through Microsoft Graph. Exchange Server installed on premises is outside the first release scope. See the [product brief](docs/product/brief.md) and [Microsoft Graph research](docs/research/microsoft-graph.md).

## Start the desktop shell

Install Rust stable with `rustfmt` and `clippy`. Einfach Mail's locked GPUI Kit dependency graph requires Rust 1.95 or newer; see the [MSRV evidence](docs/research/gpui-kit.md#minimum-rust-version). Install the native build tools for your platform as described in [development setup](docs/project/setup.md).

Start the native desktop app from the repository root:

```sh
cargo run --locked --package einfach-mail-desktop --bin einfach-mail
```

`Cargo.lock` pins GPUI Kit and its transitive dependencies. Keep it under version control and do not edit it by hand. When intentionally changing dependencies, update and review the lockfile before committing it. The initial window is a native shell and does not connect to an account.

## Check Rust changes

Run these commands from the repository root:

```sh
cargo depgraph-check check
cargo deny check bans
bash scripts/check-no-tauri-target-dependencies.sh
cargo +1.95.0 check --workspace --all-targets --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo doc --workspace --no-deps --locked
cargo audit --deny yanked
cargo test --locked --package einfach-mail-storage-proof
cargo test --workspace --locked
```

Install `cargo-depgraph-check`, `cargo-deny`, and `cargo-audit` as described in [development setup](docs/project/setup.md). The dependency boundary check compares direct internal workspace edges with `depgraph-rules.toml`. `cargo deny check bans` blocks Tauri runtime and WebView crates. The graph script checks active dependencies separately for each supported target, including the target-disjoint notification helper in `Cargo.lock`. CI also checks the declared Rust 1.95 minimum on all supported operating systems. Formatting, Clippy, documentation, dependency policy, and tests run in [CI](.github/workflows/ci.yml); RustSec auditing runs in its [own workflow](.github/workflows/rustsec-audit.yml) on pull requests, pushes to `main`, and weekly. The audit fails if a locked dependency is yanked. Provider changes also need a sanitized fixture or test-tenant evidence; a successful build does not verify Microsoft Graph behavior.

## Project map

- `apps/desktop/` contains the GPUI application and native views.
- `crates/domain/` currently contains account-scoped opaque identifier types. Mail, calendar, and account entities are not implemented yet.
- `crates/application/` will hold user workflows and provider/storage ports.
- `tools/storage-proof/` contains test-only SQLCipher and Windows Credential Manager proofs; it exposes no application API.
- `docs/product/` contains product scope.
- `docs/architecture/` describes module ownership and data flow.
- `docs/decisions/` contains accepted and proposed architecture decisions plus the ADR template.
- `docs/project/` contains setup, workflow, roadmap, and current status.
- `docs/project/workflow.md` defines how issues, implementation, verification, and decisions are handled.
- `docs/research/` records source material and integration findings.
- `docs/research/microsoft-identity.md` records Microsoft Entra desktop sign-in and broker constraints.
- `docs/project/dependency-review.md` records crate candidates and the evidence needed before adoption.
- `docs/research/gpui-kit.md` records the UI toolkit version, platform requirements, and optional email renderer limits.
- `.agents/skills/` contains pinned rust and GPUI Kit coding and design guidance; `skills-lock.json` records the upstream repository and content hashes.
- `AGENTS.md` is the required workflow and code guidance for contributors and coding agents.
- `depgraph-rules.toml` lists the allowed direct dependencies between workspace crates; CI checks it with `cargo-depgraph-check`.
- `deny.toml` blocks Tauri runtime and WebView crates. CI also checks active target graphs with `scripts/check-no-tauri-target-dependencies.sh`.
- `.github/` contains issue and pull request templates, cross-platform Rust CI, RustSec auditing, and Dependabot configuration.

## Current project state

Read [project status](docs/project/status.md) before planning work. Use the [roadmap](docs/project/roadmap.md) to choose the next milestone. Record lasting architecture choices as numbered ADRs, not as chat-only notes.
