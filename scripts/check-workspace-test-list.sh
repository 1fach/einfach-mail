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
trap 'rm -f "$actual_list"' EXIT

cargo test --workspace --locked -- --list > "$actual_list"
if cmp -s "$expected_list" "$actual_list"; then
    printf 'Workspace test listing matches %s\n' "$expected_list"
else
    diff -u "$expected_list" "$actual_list"
    exit 1
fi
