#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if (($# != 1)); then
    printf 'Usage: %s EXPECTED_TEST_LIST\n' "$0" >&2
    exit 2
fi

expected_list="$1"
if [[ ! -f "$expected_list" ]]; then
    printf 'Expected test listing does not exist: %s\n' "$expected_list" >&2
    exit 2
fi

actual_list="$(mktemp)"
expected_names="$(mktemp)"
actual_names="$(mktemp)"
trap 'rm -f "$actual_list" "$expected_names" "$actual_names"' EXIT

cargo +1.95.0 test --workspace --locked -- --list > "$actual_list"

test_names() {
    awk '/: test$/ { name = $1; sub(/:$/, "", name); sub(/^.*::/, "", name); print name }' "$1" |
        LC_ALL=C sort
}

test_names "$expected_list" > "$expected_names"
test_names "$actual_list" > "$actual_names"

if cmp -s "$expected_names" "$actual_names"; then
    test_count="$(wc -l < "$actual_names" | tr -d '[:space:]')"
    printf 'Workspace test function names match baseline (%s tests)\n' "$test_count"
else
    diff -u "$expected_names" "$actual_names"
    exit 1
fi
