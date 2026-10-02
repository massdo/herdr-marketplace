#!/bin/sh
# Offline release-policy checks in a disposable repository.
set -eu
ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
trap 'exit 1' HUP INT TERM

mkdir -p "$WORK/scripts"
cp "$ROOT/scripts/release-version.sh" "$WORK/scripts/"
cd "$WORK"
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
git -c init.defaultBranch=main init -q

versions() {
  printf '[package]\nname = "herdr-marketplace"\nversion = "%s"\n' "$1" > Cargo.toml
  printf 'id = "herdr-marketplace"\nversion = "%s"\n' "$2" > herdr-plugin.toml
  printf '[[package]]\nname = "another-crate"\nversion = "99.0.0"\n[[package]]\nname = "herdr-marketplace"\nversion = "%s"\n' "$3" > Cargo.lock
}

reject() {
  reason=$1
  shift
  if sh scripts/release-version.sh "$@" > output 2> error; then
    echo "expected rejection: $reason" >&2
    exit 1
  fi
  grep -qF "$reason" error || { cat error >&2; exit 1; }
}

versions 0.3.0 0.3.0 0.3.0
git add Cargo.toml herdr-plugin.toml Cargo.lock
git -c user.name=test -c user.email=test@example.invalid -c commit.gpgsign=false commit -qm base
base=$(git rev-parse HEAD)
[ "$(sh scripts/release-version.sh)" = 0.3.0 ]
reject 'main requires a version newer than 0.3.0' "$base"

# Documentation changes still require a release.
echo documentation > README.md
reject 'main requires a version newer than 0.3.0' "$base"

versions 0.2.9 0.2.9 0.2.9
reject 'main requires a version newer than 0.3.0' "$base"

versions 0.3.1 0.3.0 0.3.1
reject 'versions disagree' "$base"
versions 0.3.1 0.3.1 0.3.0
reject 'versions disagree' "$base"

for invalid in '' 0.03.1 0.3.1-rc.1 0.3.1+build 0.3; do
  versions "$invalid" "$invalid" "$invalid"
  reject 'expected a stable MAJOR.MINOR.PATCH version' "$base"
done

for next in 0.3.1 0.10.0 1.0.0; do
  versions "$next" "$next" "$next"
  [ "$(sh scripts/release-version.sh "$base")" = "$next" ]
done

versions 0.3.1 0.3.1 0.3.1
git -c tag.gpgsign=false tag v0.3.1 "$base"
reject 'tag v0.3.1 already exists' "$base"
# The release workflow can validate the same version when resuming a draft.
[ "$(sh scripts/release-version.sh)" = 0.3.1 ]

git -c user.name=test -c user.email=test@example.invalid -c tag.gpgsign=false tag -a v0.4.0 -m release "$base"
versions 0.4.0 0.4.0 0.4.0
reject 'tag v0.4.0 already exists' "$base"

if sh scripts/release-version.sh missing-commit > output 2> error; then
  echo 'accepted a missing base commit' >&2
  exit 1
fi
echo 'release-version: all checks passed'
