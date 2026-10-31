#!/usr/bin/env bash

set -euo pipefail

readonly forbidden_pattern='^(tauri([_-][^[:space:]]+)?|wry|gpui-webview)[[:space:]]'
targets=("$@")
if ((${#targets[@]} == 0)); then
	if ! command -v rustc >/dev/null 2>&1; then
		printf '%s\n' "rustc is not on PATH. Install Rust and add its bin directory to PATH." >&2
		exit 127
	fi

	linux_host_target="$(rustc -vV | sed -n 's/^host: //p')"
	if [[ "$linux_host_target" != *-linux-* ]]; then
		printf 'The active Rust host target is not Linux: %s\n' "$linux_host_target" >&2
		exit 2
	fi

	targets=("$linux_host_target")
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
