#!/usr/bin/env python3
"""Reject unsupported Rust module paths under apps/ and crates/."""

import re
import sys
from pathlib import Path


REPOSITORY = Path(__file__).resolve().parent.parent
SOURCE_ROOTS = ("apps", "crates")
PATH_ATTRIBUTE = re.compile(r"#\s*\[\s*path\b")


def module_layout_violations(repository: Path) -> list[str]:
    violations = []
    for source_root_name in SOURCE_ROOTS:
        for source_file in sorted((repository / source_root_name).rglob("*.rs")):
            relative_path = source_file.relative_to(repository).as_posix()
            if source_file.name == "mod.rs":
                violations.append(
                    f"{relative_path}: mod.rs is forbidden; use foo.rs with foo/."
                )

            source_text = source_file.read_text(encoding="utf-8")
            for match in PATH_ATTRIBUTE.finditer(source_text):
                line_number = source_text.count("\n", 0, match.start()) + 1
                violations.append(
                    f"{relative_path}:{line_number}: #[path] is forbidden under "
                    "apps/ and crates/"
                )
    return violations


def main() -> int:
    violations = module_layout_violations(REPOSITORY)
    if violations:
        print("Rust module layout check failed:", file=sys.stderr)
        for violation in violations:
            print(f"  {violation}", file=sys.stderr)
        return 1

    print("Rust module layout check passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
