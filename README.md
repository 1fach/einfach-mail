# Einfach Mail

Einfach Mail is a Rust desktop mail and calendar client with Microsoft 365 integration. The native UI uses GPUI Kit. Linux is the sole development and validation target for the current milestones; Windows and macOS support follow later. `main` contains read-only mail sync, encrypted offline browsing, safe native message rendering, and a cached Linux weekly calendar agenda. Calendar sync implements bounded recovery for expired delta cursors; live Graph reset behavior remains unverified. M4 adds mail and timed-appointment actions with recoverable confirmation. Manual Linux accessibility review and live Microsoft 365 tenant verification remain open. See [project status](docs/project/status.md) and the [roadmap](docs/project/roadmap.md) for current review state.

The first provider target is Exchange Online through Microsoft Graph. Exchange Server installed on premises is outside the first release scope. See the [product brief](docs/product/brief.md) and [Microsoft Graph research](docs/research/microsoft-graph.md).

## Run the Linux desktop app

Install Rust stable with `rustfmt` and `clippy`. Einfach Mail's locked GPUI Kit dependency graph requires Rust 1.95 or newer; see the [MSRV evidence](docs/research/gpui-kit.md#minimum-rust-version). Install the Linux native build tools described in [development setup](docs/project/setup.md).

Set `EINFACH_MAIL_ENTRA_CLIENT_ID` to the public client ID from an Entra app registration to enable sign-in. Then start the app from the repository root:

```sh
export EINFACH_MAIL_ENTRA_CLIENT_ID='<public-app-client-guid>'
export EINFACH_MAIL_ENTRA_CALLBACK_PORT='<registered-loopback-port>'
cargo run --locked --package einfach-mail-desktop --bin einfach-mail
```

`Cargo.lock` pins GPUI Kit and its transitive dependencies. Keep it under version control and do not edit it by hand. When intentionally changing dependencies, update and review the lockfile before committing it. The native window opens to the account view. See [development setup](docs/project/setup.md) for sign-in requirements and limits on live testing.

## Check changes locally

Run the Linux project gate from the repository root before updating a branch:

```sh
bash scripts/check-linux-local.sh
```

The script checks dependency boundaries and bans, the Linux target graph, Rust 1.95 compilation, formatting, Clippy, documentation, RustSec, and workspace tests. It checks for pinned tools first and prints install commands when any are missing. It does not invoke `gh` or start GitHub Actions.

`cargo audit` stores its advisory database under ignored `target/rustsec-advisory-db` and may fetch updates. If Perl lacks `FindBin.pm`, the script reports when it uses system OpenSSL. Workspace tests skip ignored live Secret Service checks; see [development setup](docs/project/setup.md) to run them separately.

The repository defines build, dependency, and test gates in [its Linux workflow](.github/workflows/ci.yml), with RustSec in [a separate workflow](.github/workflows/rustsec-audit.yml).

## Project map

- `apps/desktop/` contains the GPUI application and native views.
- `crates/domain/` contains provider-independent account, folder, message, calendar occurrence, and sync values.
- `crates/application/` contains account, mail, and calendar workflows; cache/provider ports; cancellation ordering; and the application-owned email sanitizer.
- `crates/providers/microsoft-graph/` contains Linux OAuth, Graph profile, mail and calendar reads, explicit plain-text sends and replies, response mapping, and Secret Service refresh-token storage.
  Its tests exercise the production Graph adapter with synthetic HTTP fixtures; live Graph behavior remains unverified.
- `crates/persistence/` owns the Linux XDG cache, SQLCipher schema and migrations, and atomic mail and calendar page commits.
- `tools/storage-proof/` contains an optional test-only Linux Secret Service and SQLCipher dependency proof. Its synthetic cache is separate from production persistence; it exposes no application API.
- `docs/product/` contains product scope.
- `docs/architecture/` describes module ownership and data flow.
- `docs/decisions/` contains accepted and proposed architecture decisions plus the ADR template.
- `docs/project/` contains setup, workflow, roadmap, and current status.
- `docs/project/workflow.md` defines how issues, implementation, verification, and decisions are handled.
- `docs/research/` records source material and integration findings.
- `docs/research/microsoft-identity.md` records Microsoft Entra desktop sign-in and broker constraints.
- `docs/project/dependency-review.md` records dependency choices and their review evidence.
- `docs/research/gpui-kit.md` records the UI toolkit version, platform requirements, and optional email renderer limits.
- `.agents/skills/` contains pinned rust and GPUI Kit coding and design guidance; `skills-lock.json` records the upstream repository and content hashes.
- `AGENTS.md` is the required workflow and code guidance for contributors and coding agents.
- `depgraph-rules.toml` lists the allowed direct dependencies between workspace crates; CI checks it with `cargo-depgraph-check`.
- `deny.toml` blocks Wry and `gpui-webview` until the optional message-body renderer passes the security review in ADR 0007.
- `.github/` contains issue and pull request templates, Linux Rust CI, RustSec auditing, and Dependabot configuration.

## Current project state

Read [project status](docs/project/status.md) before planning work. Use the [roadmap](docs/project/roadmap.md) to choose the next milestone. Record lasting architecture choices as numbered ADRs, not as chat-only notes.
