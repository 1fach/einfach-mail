#!/usr/bin/env python3
"""Reject the retired editable-event API and require its versioned replacement."""

from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parents[1]
SOURCE_ROOTS = (ROOT / "apps", ROOT / "crates")
LEGACY_PATTERNS = {
    "retired event type": re.compile(r"\bEditableCalendarEvent\b"),
    "retired event validation error": re.compile(r"\bInvalidEditableCalendarEvent\b"),
    "retired workflow error": re.compile(r"\bInvalidEditableEvent\b"),
    "retired provider method": re.compile(r"\bfn\s+editable_event\s*(?:<[^>]+>\s*)?\("),
    "retired event loader call": re.compile(r"(?:\.|::)editable_event\s*\("),
    "retired Graph event loader": re.compile(
        r"(?:\bfn\s+|\.|::)editable_calendar_event\s*\("
    ),
}


def main() -> int:
    problems = []
    for source_root in SOURCE_ROOTS:
        for path in source_root.rglob("*.rs"):
            source = path.read_text(encoding="utf-8")
            for description, pattern in LEGACY_PATTERNS.items():
                if pattern.search(source):
                    problems.append(f"{path.relative_to(ROOT)} contains a {description}")

    required_symbols = {
        ROOT / "crates/domain/src/calendar/edit.rs": r"pub struct VersionedCalendarEvent\b",
        ROOT / "crates/application/src/calendar/provider.rs": r"fn load_versioned_event\s*\(",
        ROOT / "crates/application/src/calendar/workflow.rs": r"pub async fn load_versioned_event\s*\(",
        ROOT / "crates/application/src/account/session.rs": r"fn load_versioned_event\s*<",
    }
    for path, symbol in required_symbols.items():
        if not re.search(symbol, path.read_text(encoding="utf-8")):
            problems.append(f"{path.relative_to(ROOT)} is missing the versioned event API")

    if problems:
        print("Calendar versioned-event API check failed:", file=sys.stderr)
        for problem in problems:
            print(f"- {problem}", file=sys.stderr)
        return 1

    print("Calendar versioned-event API names are consistent.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
