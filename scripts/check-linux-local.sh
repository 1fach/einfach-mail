#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if [[ "$(uname -s)" != "Linux" ]]; then
    printf '%s\n' "This check script targets Linux." >&2
    exit 2
fi

if ! command -v python3 >/dev/null 2>&1; then
    printf '%s\n' "python3 is required to run the source-map tests." >&2
    exit 127
fi

if ! command -v cargo >/dev/null 2>&1; then
    printf '%s\n' "cargo is not on PATH. Install Rust and add its bin directory to PATH." >&2
    exit 127
fi

missing_cargo_tool=0

require_cargo_tool() {
    local executable="$1"
    local install_command="$2"
    if ! command -v "$executable" >/dev/null 2>&1; then
        printf 'Required check tool is missing: %s\n' "$executable" >&2
        printf 'Install it with: %s\n' "$install_command" >&2
        missing_cargo_tool=1
    fi
}

require_cargo_tool cargo-depgraph-check "cargo install cargo-depgraph-check --version 0.1.0 --locked"
require_cargo_tool cargo-deny "cargo install cargo-deny --version 0.20.2 --locked"
require_cargo_tool cargo-audit "cargo install cargo-audit --version 0.22.2 --locked --no-default-features"
if (( missing_cargo_tool != 0 )); then
    exit 127
fi

if [[ -z "${OPENSSL_NO_VENDOR:-}" ]] && ! perl -MFindBin -e 1 >/dev/null 2>&1; then
    if command -v pkg-config >/dev/null 2>&1 && pkg-config --exists openssl; then
        export OPENSSL_NO_VENDOR=1
        printf '%s\n' "Perl FindBin.pm is unavailable; using system OpenSSL for this local run."
    else
        printf '%s\n' "Perl FindBin.pm and system OpenSSL are both unavailable." >&2
        exit 1
    fi
fi

run() {
    printf '\n==> %s\n' "$*"
    "$@"
}

run python3 -B -m unittest discover -s scripts/tests
run python3 -B scripts/check-rust-module-layout.py
run python3 -B scripts/check-calendar-versioned-api.py
run cargo depgraph-check check
run cargo deny check bans
run cargo +1.95.0 check --workspace --all-targets --locked
run cargo +1.95.0 check --package einfach-mail-desktop --features ui-preview --locked
run cargo fmt --all -- --check
run cargo clippy --workspace --all-targets --locked -- -D warnings
run cargo doc --workspace --no-deps --locked
if [[ -n "${EINFACH_RUSTSEC_DB:-}" ]]; then
    run cargo audit --db "$EINFACH_RUSTSEC_DB" --no-fetch --deny yanked
else
    run cargo audit --deny yanked
fi
run cargo test --workspace --locked

printf '\nLinux project checks passed.\n'
