---
name: rust-comments-rules
description: >-
  Write or review comments and Rustdoc in Rust source files. Use when adding,
  editing, or assessing inline comments, module docs, public API docs, or
  Rustdoc examples. Do not use for unrelated Rust implementation work.
---

# Rust Comment Rules

Apply these rules when writing or reviewing comments in Rust files. This includes
inline comments (`//`), outer documentation comments (`///`), and inner
documentation comments (`//!`). Prefer `///` and `//!` for documentation because
`rustdoc` turns them into API documentation.

This skill focuses on comment accuracy and clarity. Follow the local
`rust-best-practices` skill for Rust implementation and API decisions, and use
`rust-review` when the task is a review of Rust code.

## General Comment Rules

### Core rule

Write comments from the implementation outward. Describe what the code does or
why it needs a non-obvious constraint. Do not describe what the code should do
when the implementation does something else.

Add an inline comment only when it explains a non-obvious guard, ordering
requirement, state change, invariant, platform constraint, or unusual
implementation choice. Do not comment code that its names and structure already
explain.

Name the value or operation that the comment describes. Keep the comment close
to that code. Prefer types and executable checks for rules the compiler can
enforce; a comment alone does not enforce an invariant.

Use short, direct language as guidance, not as a strict style test. Keep Rust
symbols, API names, field names, and established technical terms exact. Prefer
one fact or action per sentence.

### Comment quality

Comments must:

- describe implemented behavior or a verified constraint;
- state a concrete action and target when describing behavior;
- include user-facing context only when the code or its consumers establish it;
- avoid invented workflows, motivations, call sites, and effects;
- avoid generic value claims, filler, and undefined abstractions.

Use an active verb and name the changed or returned value. Avoid broad verbs
such as `handles`, `manages`, and `processes` unless the comment names the exact
result.

Bad: `/// Processes rejected messages.`

Good, when the function filters and returns messages: `/// Returns messages
that the server rejected.`

For guards, state the action and the condition that permits it:

```rust
// Apply results only when this request is still current.
if request_id == state.current_request_id {
    state.results = results;
}
```

Do not replace code behavior with an assumed user benefit. Do not infer a
function's call site or purpose unless the implementation or its consumers prove
it.

## Rustdoc Rules

Apply these rules to `///` and `//!` comments. Rust signatures already show
parameter names and types. Document the contract, constraints, and behavior that
the signature does not show; do not add JSDoc-style tags such as `@param`.

### Coverage and scope

- Add useful Rustdoc to public API items: crates and modules that form the API,
  public structs, enums, traits, fields, variants, functions, methods, macros,
  type aliases, and associated items. Follow the workspace's `missing_docs`
  lint and established exceptions for generated or implementation-only items.
- Use `//!` for the crate or module containing the comment. Use `///` for the
  item that follows it.
- Add docs to private or crate-visible helpers when they contain non-obvious
  validation, normalization, mutation, asynchronous behavior, or state logic.
  Skip routine glue and obvious one-line helpers.
- For public or shared items, inspect call sites, implementations, tests, and
  consumers before writing or approving docs. Include the behavior and
  invariants that callers rely on.
- Re-read the implementation whenever code changes. Existing Rustdoc can become
  stale.

### Summaries and prose

- Start with a concise summary of the implemented behavior. Keep the first
  sentence useful on its own in rustdoc lists.
- Add prose after a blank line for constraints, invariants, defaults, or effects
  that callers need to know.
- Use exact item and field names. Link to related Rust items with intra-doc
  links such as [`Message`] or [`Self::is_empty`]. Check that each link resolves.
- Explain the meaning of an `Option` when `None` or `Some` has non-obvious
  semantics. Explain parameter constraints such as units, valid ranges, ordering,
  or ownership expectations when the type signature does not make them clear.
- Document public fields and enum variants with their meaning or contract. Put
  type-wide invariants on the type, not only on an individual field.
- Document trait contracts and implementation obligations on the trait. Add
  method-specific details where they differ. Do not repeat the same contract on
  every implementation.
- Use capitalization and punctuation. Do not restate a name when it adds no
  information; explain the contract or behavior instead.

### Results, panics, and safety

- Add a `# Errors` section when a public function, method, or trait method can
  return `Err`. State the conditions that produce each relevant error,
  including distinctions callers may need to handle.
- Add a `# Panics` section when a public function or method can panic. State the
  condition that causes the panic. Do not claim a function cannot panic unless
  the implementation supports that claim.
- Describe returned data and meaningful empty or absent cases. A `Result` or
  `Option` type in the signature does not explain what its values mean.
- For `async fn`, describe the result when callers await it. Document
  cancellation or deferred side effects only when that behavior matters to
  callers and is established by the implementation.
- If an unsafe API is present, document its caller obligations under `# Safety`.
  Cover every invariant callers must uphold for an unsafe function or trait.
  This project forbids unsafe Rust unless an ADR approves a narrow exception.
- If an ADR permits an unsafe block, put a `// SAFETY:` comment directly above
  it. State the invariant that makes the operation valid and how the code
  establishes that invariant. A block comment does not replace `# Safety` docs
  on an unsafe API.

### Examples

- Add a `# Examples` section when a public API is non-trivial or its use is not
  clear from the signature. Show why to use the API, not only how to call it.
  Keep examples in crate-level docs when repeating them on each item would add
  little.
- Treat Rust code blocks as doctests unless marked otherwise. Keep examples
  consistent with the actual API and make them compile. Use `?` or explicit
  error handling instead of `unwrap()` on fallible operations. Use hidden `#`
  lines for setup that does not help explain the example.
- Avoid examples that expose real credentials, mailbox data, or other secrets.

## Unclear Language

Bad:

`/// Coordinates the incoming event reconciliation flow.`

Good, when the implementation loads events for a selected date range:

`/// Loads events for the selected date range.`

Name the actual operation and target. Avoid stacked noun phrases, vague
pronouns, idioms, and metaphors that make the relationship between values hard
to follow.

## Common Mistakes

- **The comment restates the name:** Add the behavior or constraint callers
  need, or remove the comment.
- **The docs describe intended behavior:** Rewrite them from the current
  implementation and its tested consumer contract.
- **A private invariant appears only in prose:** Encode it in a type or check
  when Rust can enforce it.
- **A `Result` has no error conditions:** Add `# Errors` and state when callers
  receive each relevant error.
- **A panic is reachable but undocumented:** Add `# Panics` with its condition.
- **An `Option` has unclear absence semantics:** Explain what `None` means.
- **A fallible example uses `unwrap()`:** Use `?` or handle the error
  explicitly.
- **A trait's implementer contract is hidden:** Document the obligation on the
  trait or method.
- **The comment uses a vague verb:** Name the action and the value it changes
  or returns.
- **The comment claims an effect at a call site:** Confirm it from the
  implementation or a consumer before including it.
- **A public doc link is broken:** Use a resolvable intra-doc link and check it
  with rustdoc.
- **A comment survives a behavior change without review:** Rebuild it from the
  changed implementation.

## Red Flags

- The comment could describe several unrelated functions.
- The summary says what the function should do instead of what it does.
- The docs describe an inferred workflow, motivation, or call site.
- A sentence uses an unclear pronoun or does not name its target.
- A guard comment omits the condition that changes the action.
- A public `Result` has no documented error conditions.
- A reachable panic or unsafe caller obligation is missing from public docs.
- An example uses a symbol or behavior that does not exist in the implementation.
- A Rustdoc link cannot be resolved by `rustdoc`.
- A private helper has boilerplate docs that add no information.

## Quality Checklist

- The summary matches the implementation.
- Every sentence describes one behavior, constraint, effect, or return meaning.
- Names and links match the code.
- Public API docs explain the contract beyond the Rust signature.
- Errors, panics, absence cases, and relevant async effects are documented.
- Examples compile and use the real API.
- Comments contain no invented call sites, motivations, effects, or generic
  value claims.
- Existing comments were checked against the current implementation.

## Writing Recipe

1. Read the implementation and identify the behavior or constraint that needs
   explanation.
2. For a caller-facing item, inspect its consumers, tests, and implementations.
3. Write a concise summary from the behavior that the code proves.
4. Add only the details callers need: constraints, mutation, returned values,
   error conditions, panic conditions, async effects, or trait obligations.
5. Add links or an example when they help callers use the API.
6. Check each noun and claim against the implementation and consumers.
7. Remove filler, repeated signatures, unsupported context, and stale claims.
8. When the task calls for documentation checks, run the repository's Rustdoc
   checks, such as `cargo doc --no-deps`, and fix broken links, doctests, or
   `missing_docs` diagnostics.
