# Instructions for contributors and coding agents

This file applies to the whole repository. Follow it before changing code or project plans.

## Read the current project state

Before starting work, read:

1. `docs/project/status.md` for the current milestone and blockers.
2. `docs/product/brief.md` for the product goal and scope.
3. `docs/architecture/system.md` for component ownership and data flow.
4. The relevant records in `docs/decisions/` for accepted constraints.

Read nearby source and tests before editing them. Treat checked-in files as the source of truth.

## Keep project memory in one place

- Put the current milestone, active risks, and next actions in `docs/project/status.md`.
- Put product scope and user outcomes in `docs/product/brief.md`.
- Record lasting architectural choices in a numbered ADR under `docs/decisions/`.
- Keep implementation plans and acceptance criteria in issues once the repository has a remote. Do not copy issue plans into `status.md`.
- Update `docs/project/roadmap.md` when a milestone changes. Mark completed work only after checking the resulting code or behavior.
- Do not add personal agent memory files. Durable project facts belong in the files above.
- The pinned rust and GPUI Kit agent guidance lives in `.agents/skills/`. `skills-lock.json` records its upstream source and SHA-256 hashes for the copied files. Do not edit copied skill files by hand.

## Look for a library before writing general-purpose code

When a need comes up, first look for a maintained Rust library that fits it. For general-purpose capabilities such as protocols, parsing, sanitization, storage, and operating-system integration, use a suitable library instead of writing that capability ourselves. Write custom code only when the review shows that no good library fits the requirements, and record why. Check current maintenance, platform support, license, MSRV, security history, API fit, features, and dependency cost. Compare alternatives for authentication, storage, network access, and untrusted content. Use `docs/project/dependency-review.md` to record the evidence. Keep Hey Mail-specific product rules in the domain and application crates. Record major dependency choices in an ADR. Commit `Cargo.lock` for this application.

## Use project skills

- Use the relevant pstack skill for design and investigation. Use `architect` for new boundaries, ownership, and data flow. Use `how` and `why` to inspect existing behavior and rationale. Use `blast-radius` for changes that cross callers, persistence, providers, or lifecycle boundaries. Apply the relevant `principle-*` skills when their design problem appears.
- Use `rust-best-practices` for every Rust change. Do not force unrelated skills onto a task.
- Before GPUI framework, lifecycle, state, or component work, read `.agents/skills/gpui-kit/SKILL.md` and the relevant coding-guide sections. Before designing or changing visible UI, read `.agents/skills/gpui-kit-design-guides/SKILL.md` and its required design-guide sections.

## Work in small, reviewable changes

1. Identify the user outcome and acceptance criteria before editing.
2. Check whether the task changes product scope, security, provider behavior, data ownership, or a public boundary. Write or update an ADR before implementing such a change.
3. Keep each change within one user outcome. Avoid unrelated cleanup.
4. Update docs when behavior, setup, a decision, or project status changes.
5. Run the checks in `README.md` that apply to the change. Add regression tests for changed behavior.
6. Report the files changed, checks run, and any unverified behavior in the pull request.

Do not claim that a feature works because it compiles. For provider work, verify the real request and response path against a test tenant or a recorded, sanitized fixture. Never place tokens, message bodies, addresses, or calendar details in fixtures, issues, logs, or pull request text.

## Keep the dependency direction

```text
desktop UI -> application -> domain
                    ^
                    |
              infrastructure
```

- `crates/domain` owns mail and calendar concepts and their invariants. It must not depend on GPUI, Microsoft Graph, HTTP, SQLite, or frontend code.
- `crates/application` owns user workflows and ports. It depends on domain concepts, not on concrete providers or storage engines.
- `apps/desktop` owns GPUI lifecycle, native views, keyboard actions, and operating-system integration. It calls application workflows and contains no provider or persistence rules.
- Provider and persistence adapters translate external formats into application and domain types. Parse and validate external data at those boundaries.
- Keep UI state in GPUI entities and views. Views must not call Microsoft Graph or access the database directly.

Do not add an abstraction that only forwards calls. Add a crate when it owns a real rule or boundary.

## Rust rules

- Use the `rust-best-practices` skill for every Rust change.
- Prefer concrete types, private fields, `Result` for failures, and newtypes for values that have different meanings.
- Keep parsing and validation at external boundaries. Internal functions should receive valid domain values.
- Do not use `unwrap()` on fallible paths. Use `expect("reason")` only for an invariant that cannot be expressed as a `Result`.
- Avoid public APIs until another crate needs them. Keep domain types independent of Graph JSON and database rows.
- Do not add `unsafe` code. The workspace forbids it unless a future ADR approves a narrow, justified exception.
- Put lint configuration in `Cargo.toml`. Do not add `#![deny(warnings)]`.

## Mail and calendar security

- Treat Graph responses, downloaded attachments, email bodies, calendar descriptions, and saved data as untrusted input.
- Search for a maintained parser or sanitizer before writing one. Remove scripts, remote loads, active content, and unsafe navigation from rendered email. Keep remote images and other remote content blocked by default.
- Keep the main application UI native GPUI. If a browser engine is needed for rich email, isolate it to the message body and make its use a separate architecture decision. Do not expose app commands, tokens, or mailbox data to message content.
- Keep OAuth tokens out of views, logs, SQLite rows, source files, and test fixtures. Use operating-system credential storage after the authentication design is approved.
- Request delegated Microsoft Graph permissions for the signed-in user. Do not request tenant-wide application permissions for this desktop client.
- Use Microsoft Graph v1.0 for Exchange Online. Do not add Exchange Web Services calls for Exchange Online.
- Never use real mailbox data in automated tests. Use synthetic fixtures with personal data removed.

## UI and accessibility

- Keep mail, calendar, and account settings in the same native desktop application.
- Prefer keyboard-accessible controls with visible focus and clear loading, empty, error, and offline states.
- Do not add controls that look interactive until they perform an action or explain why they are unavailable.
- Follow the accepted GPUI Kit decision. For a new UI framework or navigation model, compare working options and record the choice before implementation.

## Pull request gate

Every pull request must:

- link to one issue or explain why no issue is needed;
- state the user-visible outcome;
- list behavior and security risks;
- update the relevant docs and ADRs;
- pass formatting, Clippy, and workspace tests in CI;
- include evidence for any UI or provider behavior it changes.

Use `.github/PULL_REQUEST_TEMPLATE.md` and the issue templates. Do not merge with a failing required check.
