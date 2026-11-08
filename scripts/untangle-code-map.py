#!/usr/bin/env python3
"""Print a compact Rust source and recent cross-component change map.

Co-change counts are review clues, not proof that two files share an invariant.
Use --declarations PATH to list item declaration lines from one file.
"""

import argparse
import re
import subprocess
from collections import Counter
from itertools import combinations
from pathlib import Path


REPOSITORY = Path(__file__).resolve().parent.parent
COMMIT_ID = re.compile(r"[0-9a-f]{40}")
ITEM_DECLARATION = re.compile(
    r"^(?:pub(?:\([^)]*\))?\s+)?(?:(?:async|const|unsafe)\s+)*"
    r"(?:fn|struct|enum|trait|type|mod|impl)\b"
)
TEST_ATTRIBUTE = re.compile(
    r"^\s*#\[\s*(?:[A-Za-z_][A-Za-z0-9_]*\s*::\s*)*test(?:\s*\(|\s*\])"
)


def git_output(*arguments):
    result = subprocess.run(
        ["git", "-C", str(REPOSITORY), *arguments],
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout


def tracked_rust_files():
    paths = git_output("ls-files", "-z", "--", "apps", "crates").split("\0")
    return [REPOSITORY / path for path in paths if path.endswith(".rs")]


def count_test_attributes(lines):
    return sum(bool(TEST_ATTRIBUTE.match(line)) for line in lines)


def component(path):
    parts = Path(path).parts
    if parts[:2] == ("crates", "providers"):
        return "/".join(parts[:3])
    return "/".join(parts[:2])


def source_commits_from_log(output):
    commits = []
    current_paths = set()

    for line in output.splitlines():
        if COMMIT_ID.fullmatch(line):
            if current_paths:
                commits.append(current_paths)
            current_paths = set()
        elif line.endswith(".rs"):
            current_paths.add(line)

    if current_paths:
        commits.append(current_paths)
    return commits


def recent_file_changes(commit_count):
    output = git_output(
        "log",
        f"-n{commit_count}",
        "--name-only",
        "--format=%H",
        "--",
        "apps",
        "crates",
    )
    return source_commits_from_log(output)


def source_inventory(files):
    inventory = []
    for path in files:
        contents = path.read_text(encoding="utf-8")
        lines = contents.splitlines()
        inventory.append(
            {
                "path": path.relative_to(REPOSITORY).as_posix(),
                "lines": len(lines),
                "items": sum(
                    bool(ITEM_DECLARATION.match(line.strip())) for line in lines
                ),
                "tests": count_test_attributes(lines),
            }
        )
    return inventory


def print_declarations(path, files):
    resolved = path if path.is_absolute() else REPOSITORY / path
    resolved = resolved.resolve()
    tracked_files = {file.resolve() for file in files}
    if resolved not in tracked_files:
        raise ValueError(f"not a tracked Rust file under apps/ or crates/: {path}")

    print(f"Item declaration lines: {resolved.relative_to(REPOSITORY)}")
    for line_number, line in enumerate(resolved.read_text(encoding="utf-8").splitlines(), 1):
        declaration = line.strip()
        if ITEM_DECLARATION.match(declaration):
            print(f"{line_number}: {declaration}")


def print_source_inventory(inventory, top):
    print(f"Tracked Rust source files: {len(inventory)}")
    print("\nLargest files (lines, item declarations, test attributes):")
    for source_file in sorted(
        inventory, key=lambda entry: (-entry["lines"], entry["path"])
    )[:top]:
        print(
            f"{source_file['lines']:5} lines  {source_file['items']:3} items  "
            f"{source_file['tests']:3} tests  {source_file['path']}"
        )


def file_and_cochange_counts(commits):
    file_changes = Counter()
    co_changes = Counter()
    for source_commit_paths in commits:
        file_changes.update(source_commit_paths)
        for left_path, right_path in combinations(sorted(source_commit_paths), 2):
            if component(left_path) != component(right_path):
                co_changes[(left_path, right_path)] += 1
    return file_changes, co_changes


def print_recent_file_changes(file_changes, source_commit_count, top):
    print(f"\nMost changed files in the last {source_commit_count} source commits:")
    for path, count in file_changes.most_common(top):
        print(f"{count:4} commits  {path}")


def print_cross_component_changes(co_changes, source_commit_count, min_cochange, top):
    print(
        f"\nCross-component co-changes in the last {source_commit_count} source commits "
        f"(at least {min_cochange} commits):"
    )
    rows = sorted(
        (
            (count, left, right)
            for (left, right), count in co_changes.items()
            if count >= min_cochange
        ),
        key=lambda item: (-item[0], item[1], item[2]),
    )[:top]
    if not rows:
        print("No pairs met the threshold.")
    for count, left, right in rows:
        print(f"{count:4} commits  {left} <> {right}")


def print_report(inventory, commits, top, min_cochange):
    print_source_inventory(inventory, top)
    file_changes, co_changes = file_and_cochange_counts(commits)
    print_recent_file_changes(file_changes, len(commits), top)
    print_cross_component_changes(co_changes, len(commits), min_cochange, top)


def positive_integer(value):
    number = int(value)
    if number < 1:
        raise argparse.ArgumentTypeError("must be greater than zero")
    return number


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--commits", type=positive_integer, default=100)
    parser.add_argument("--top", type=positive_integer, default=20)
    parser.add_argument("--min-cochange", type=positive_integer, default=3)
    parser.add_argument("--declarations", type=Path)
    arguments = parser.parse_args()

    files = tracked_rust_files()
    if arguments.declarations:
        try:
            print_declarations(arguments.declarations, files)
        except ValueError as error:
            parser.error(str(error))
        return

    inventory = source_inventory(files)
    commits = recent_file_changes(arguments.commits)
    print_report(inventory, commits, arguments.top, arguments.min_cochange)


if __name__ == "__main__":
    main()
