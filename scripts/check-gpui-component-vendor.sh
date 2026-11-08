#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
vendor_root="$repo_root/vendor/gpui-component"
patch_path="$repo_root/vendor/patches/gpui-component-date-picker-accessibility.patch"

for tool in cargo curl diff git jq sha256sum tar; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        printf 'Required check tool is missing: %s\n' "$tool" >&2
        exit 127
    fi
done

temporary_root="$(mktemp -d "${TMPDIR:-/tmp}/einfach-gpui-component.XXXXXX")"
trap 'rm -rf -- "$temporary_root"' EXIT

cp -- "$vendor_root/Cargo.toml" "$temporary_root/Cargo.toml"
cargo read-manifest --manifest-path "$temporary_root/Cargo.toml" \
    | jq --exit-status --raw-output 'select(.name == "gpui-component") | .version' \
    > "$temporary_root/version"
read -r version < "$temporary_root/version"
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    printf 'Invalid gpui-component package version: %s\n' "$version" >&2
    exit 1
fi

gpui_kit_version="$(cargo read-manifest --manifest-path "$repo_root/apps/desktop/Cargo.toml" \
    | jq --exit-status --raw-output \
        '[.dependencies[] | select(.name == "gpui-kit") | .req | sub("^[^0-9]*"; "")] | unique | select(length == 1) | .[0]')"
if [[ "$version" != "$gpui_kit_version" ]]; then
    printf 'Vendored gpui-component %s does not match GPUI Kit %s.\n' \
        "$version" "$gpui_kit_version" >&2
    exit 1
fi

index_path="$temporary_root/gpui-component-index.jsonl"
archive="$temporary_root/gpui-component-$version.crate"
package_root="$temporary_root/package"
curl --fail --silent --show-error https://index.crates.io/gp/ui/gpui-component \
    --output "$index_path"
jq --exit-status --raw-output --arg version "$version" \
    'select(.vers == $version) | .cksum | strings | select(test("^[0-9a-f]{64}$"))' \
    "$index_path" > "$temporary_root/checksum"
read -r checksum < "$temporary_root/checksum"

curl --fail --location --silent --show-error \
    "https://static.crates.io/crates/gpui-component/gpui-component-$version.crate" \
    --output "$archive"
printf '%s  %s\n' "$checksum" "$archive" | sha256sum --check
mkdir -- "$package_root"
tar --extract --gzip --strip-components=1 --file "$archive" --directory "$package_root"
rm -f -- "$package_root/Cargo.lock" "$package_root/.cargo_vcs_info.json"

(
    cd -- "$package_root"
    git apply --check "$patch_path"
    git apply "$patch_path"
)

if ! diff --recursive --brief "$package_root" "$vendor_root"; then
    printf 'Vendored gpui-component differs from the verified %s crate plus the local patch.\n' "$version" >&2
    exit 1
fi

printf 'Vendored gpui-component matches crates.io %s plus the local DatePicker accessibility patch.\n' "$version"
