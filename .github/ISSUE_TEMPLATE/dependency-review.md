---
name: Dependency review
about: Evaluate a Rust crate before adding it
title: "deps: "
labels: []
assignees: []
---

## Need

What user requirement or system boundary needs a library?

## Candidates

List the crate name, current version, repository, and at least one alternative.

## Review

- [ ] Maintenance and latest release checked
- [ ] License and platform support checked
- [ ] MSRV and required Rust targets checked
- [ ] Security advisories and parser or protocol risks checked
- [ ] Default and optional features checked
- [ ] Direct and transitive dependency cost checked
- [ ] API fit checked against a real project scenario
- [ ] Integration proof added with synthetic data

## Decision

Explain why the chosen crate fits, or why the project needs custom code because no suitable crate fits.

Link the ADR if this choice changes an accepted architecture or security boundary.
