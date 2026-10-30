# Project workflow

## Track work

After a Git remote is configured, create one issue for each user-visible outcome or focused engineering task. An issue needs:

- the user problem or engineering reason;
- acceptance criteria written as observable behavior;
- the affected milestone;
- account, platform, privacy, and sync risks when relevant.

Do not put secrets or real mail, calendar, or account data in an issue. Use synthetic examples.

## Make a change

1. Read `AGENTS.md`, `docs/project/status.md`, the product brief, architecture map, and relevant ADRs.
2. Confirm the issue's acceptance criteria. Split work that spans independent user outcomes.
3. Review maintained libraries before writing infrastructure. Record the comparison in [dependency review](dependency-review.md). Do not add a crate until the review shows that it fits the requirement.
4. Write an ADR before code if the work changes an accepted boundary, provider, permission scope, persistence model, or security rule.
5. Implement one vertical slice. Keep the user outcome visible at the boundary.
6. Add behavior tests and sanitized provider fixtures. Do not test against a personal mailbox in CI.
7. Run format, Clippy, and workspace tests. Use a test tenant for provider behavior that fixtures cannot prove.
8. Update setup, architecture, research, roadmap, or status docs when their facts change.
9. Open a pull request with user impact, dependency choices, implementation notes, security risks, and actual check results.

## Decide when work is complete

Close an issue only when every acceptance criterion has evidence. A successful compiler run does not prove correct synchronization, permissions, accessibility, or UI behavior. Keep unverified items open and state what evidence is missing.

## Record decisions

Use the format in `docs/decisions/README.md`. Accepted records are historical. If a decision changes, add a new ADR that names the earlier record and explains the evidence that changed the choice.
