# Hey Mail

Hey Mail is a working name for a Rust desktop mail and calendar client with Microsoft 365 integration. The native UI uses GPUI Kit. Account sign-in, mail, calendar, and synchronization are not implemented yet.

The first provider target is Exchange Online through Microsoft Graph. Exchange Server installed on premises is outside the first release scope. See the [product brief](docs/product/brief.md) and [Microsoft Graph research](docs/research/microsoft-graph.md).

## Start the desktop shell

Install Rust stable with `rustfmt` and `clippy`. GPUI Kit currently needs Rust 1.92 or newer. Install the native build tools for your platform as described in [development setup](docs/project/setup.md).

Start the native desktop app from the repository root:

```sh
cargo run --locked --package hey-mail-desktop
```

`Cargo.lock` pins GPUI Kit and its transitive dependencies. Keep it under version control and do not edit it by hand. When intentionally changing dependencies, update and review the lockfile before committing it. The initial window is a native shell and does not connect to an account.

## Check Rust changes

Run these commands from the repository root:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

The same gates run in [GitHub Actions](.github/workflows/ci.yml). Provider changes also need a sanitized fixture or test-tenant evidence; a successful build does not verify Microsoft Graph behavior.

## Project map

- `apps/desktop/` contains the GPUI application and native views.
- `crates/domain/` will hold provider-independent mail and calendar types.
- `crates/application/` will hold user workflows and provider/storage ports.
- `docs/product/` contains product scope.
- `docs/architecture/` describes module ownership and data flow.
- `docs/decisions/` contains accepted and proposed architecture decisions plus the ADR template.
- `docs/project/` contains setup, workflow, roadmap, and current status.
- `docs/research/` records source material and integration findings.
- `docs/project/dependency-review.md` records crate candidates and the evidence needed before adoption.
- `docs/research/gpui-kit.md` records the UI toolkit version, platform requirements, and optional email renderer limits.
- `.agents/skills/` contains pinned rust and GPUI Kit coding and design guidance; `skills-lock.json` records the upstream repository and content hashes.
- `AGENTS.md` is the required workflow and code guidance for contributors and coding agents.

## Current project state

Read [project status](docs/project/status.md) before planning work. Use the [roadmap](docs/project/roadmap.md) to choose the next milestone. Record lasting architecture choices as numbered ADRs, not as chat-only notes.
