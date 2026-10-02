#!/bin/sh
# Print the stable version shared by the three manifests. With a base commit,
# also require a newer version and an unused tag before merging into main.
set -eu
cd "$(dirname "$0")/.."

fail() {
  echo "release: $*" >&2
  exit 1
}

package_version() {
  awk '/^\[/ { pkg = ($0 == "[package]") } pkg && /^version *= *"/ { sub(/^version *= *"/, ""); sub(/".*/, ""); print; exit }'
}

stable_version() {
  echo "$1" | grep -Eq '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'
}

[ "$#" -le 1 ] || fail "usage: sh scripts/release-version.sh [BASE_COMMIT]"
version=$(package_version < Cargo.toml)
manifest=$(awk '/^\[/ { exit } /^version *= *"/ { sub(/^version *= *"/, ""); sub(/".*/, ""); print; exit }' herdr-plugin.toml)
lock=$(awk '/^\[\[package\]\]$/ { own = 0 } $0 == "name = \"herdr-marketplace\"" { own = 1 } own && /^version = "/ { sub(/^version = "/, ""); sub(/".*/, ""); print; exit }' Cargo.lock)
stable_version "$version" || fail "expected a stable MAJOR.MINOR.PATCH version, got '$version'"
[ "$version" = "$manifest" ] && [ "$version" = "$lock" ] ||
  fail "versions disagree: Cargo.toml=$version herdr-plugin.toml=$manifest Cargo.lock=$lock"

if [ "$#" -eq 1 ]; then
  base_manifest=$(git show "$1:Cargo.toml")
  base=$(printf '%s\n' "$base_manifest" | package_version)
  stable_version "$base" || fail "cannot read a stable version at base commit $1"
  awk -v current="$version" -v base="$base" 'BEGIN {
    split(current, c, "."); split(base, b, ".")
    for (i = 1; i <= 3; i++) {
      if (c[i] + 0 > b[i] + 0) exit 0
      if (c[i] + 0 < b[i] + 0) exit 1
    }
    exit 1
  }' || fail "main requires a version newer than $base, got $version (including documentation-only changes)"
  if git show-ref --verify --quiet "refs/tags/v$version"; then
    fail "tag v$version already exists; choose a new version"
  fi
fi

printf '%s\n' "$version"
