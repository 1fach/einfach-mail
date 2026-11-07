# ADR 0048: Redact sensitive domain Debug output

- Status: Accepted
- Date: 2026-11-07
- Owners: Einfach Mail maintainers

## Context

Several domain values derive `Debug` while containing account labels, mailbox addresses, provider-issued identifiers, folder names, message subjects, received timestamps, or sparse patch contents. A calendar time-zone identifier also prints its raw value. Formatting a containing value can expose those fields even when the outer workflow has no logging behavior. The repository audit found no production logging sink, so this is an exposure capability rather than an observed data leak.

Mail bodies, opaque delta links, send drafts, and most calendar event details already use redacted `Debug` implementations. Nested account and mail values must follow the same boundary so a safe outer formatter cannot reveal sensitive values indirectly. The time-zone identifier has no `Debug` consumer; functional code reads it through its typed accessor.

## Decision

Redact account and mail identity and content fields and the calendar time-zone identifier at their domain value types. Redact provider-issued identifiers in their identifier newtypes so every derived or manual outer formatter inherits the safe output. Preserve non-sensitive state and field names where useful. Do not change runtime values, accessors, equality, hashing, or persistence/provider serialization.

Implement targeted `Debug` formatting with `std::fmt` for sensitive domain values. Redact provider-issued IDs in the identifier macro so every outer formatter inherits the safe output. Retain explicit `Debug` implementations for types that already need custom state-aware output. Do not add a formatter dependency for this small, fixed policy.

Add synthetic-marker tests that format direct values and nested messages, folders, delta pages, pending send records, and the calendar time-zone identifier. The tests prove that account IDs, mailbox data, folder names, subjects, received timestamps, calendar time-zone IDs, and sparse patch values do not appear in diagnostics.

## Alternatives

| Option | Benefits | Costs and risks |
| --- | --- | --- |
| Use `std::fmt::Debug` implementations for the sensitive values | Adds no dependency; gives each domain value explicit control over its output; nested values remain composable. | Requires a short implementation for each type whose raw fields must be hidden. |
| Use `derive_more::Debug` field formatting | Redaction attributes would sit beside their fields, and the crate is already present transitively through GPUI. | Enabling its `debug` feature activates `unicode-xid` 0.2.6, a new resolved package solely for formatting; the standard library already covers this narrow need. |
| Wrap every sensitive string and identifier in a secrecy type | Makes sensitivity explicit in field types. | Changes constructors, accessors, equality, hashing, persistence conversions, and provider mappings for data that is not secret storage material. |

## Consequences

- Account and mail values no longer print identifying content or provider IDs through `Debug` output, and calendar time-zone IDs are redacted.
- Existing safe diagnostic state remains available where it does not contain user data.
- Other calendar event and cursor redaction and Graph token handling remain owned by their existing types and are not redesigned here.
- Automated tests use synthetic values only. The full Linux local gate checks formatting, lint, documentation, dependency policy, RustSec advisories, and workspace behavior.
- Debug output is not a supported serialization format; callers that need a value must use the existing typed accessors.

## Sources

- [Issue #104: Redact sensitive domain Debug output](https://github.com/1fach/einfach-mail/issues/104)
- [`derive_more` 2.1.1 Debug documentation](https://docs.rs/derive_more/2.1.1/derive_more/derive.Debug.html)
- [`derive_more-impl` 2.1.1 feature manifest](https://docs.rs/crate/derive_more-impl/2.1.1/source/Cargo.toml)
- [`derive_more` upstream repository](https://github.com/JelteF/derive_more)
