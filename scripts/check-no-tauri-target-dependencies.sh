#!/usr/bin/env bash

set -euo pipefail

readonly forbidden_pattern='^(tauri([_-][^[:space:]]+)?|wry|gpui-webview)[[:space:]]'
readonly supported_targets=(
	x86_64-unknown-linux-gnu
	x86_64-pc-windows-msvc
	x86_64-apple-darwin
	aarch64-apple-darwin
)

targets=("$@")
if ((${#targets[@]} == 0)); then
	targets=("${supported_targets[@]}")
fi

for target in "${targets[@]}"; do
	package_tree="$(cargo tree --locked --workspace --all-features --edges all --target "$target" --prefix none --format '{p}')"
	forbidden_packages="$(grep -E "$forbidden_pattern" <<<"$package_tree" || true)"

	if [[ -n "$forbidden_packages" ]]; then
		printf 'Forbidden Tauri or WebView packages in the %s dependency graph:\n%s\n' \
			"$target" "$forbidden_packages" >&2
		exit 1
	fi

	printf 'No Tauri or WebView packages in the %s dependency graph.\n' "$target"
done
