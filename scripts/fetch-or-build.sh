#!/bin/sh
# Use the prebuilt binary of release v<version> only when it was built from
# this exact checkout; otherwise build from source and keep only the binary.
# Adapted from herdr-npm v0.1.0, itself adapted from herdr-sidebar 0.13.0
# scripts/fetch-or-build.sh (MIT, see NOTICE).
set -u

repo=massdo/herdr-marketplace
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd) || exit 1
release_dir="$root/target/release"
source_build="$root/target/source-build"
tmp=""

cleanup() {
  if [ -n "$tmp" ]; then
    rm -rf "$tmp"
    tmp=""
  fi
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

# Herdr 0.9.1 hides the output of a successful build, so status lines also go
# to $HERDR_MARKETPLACE_BUILD_LOG when it is set. That copy never changes the
# outcome.
report() {
  echo "herdr-marketplace: $1"
  if [ -n "${HERDR_MARKETPLACE_BUILD_LOG:-}" ]; then
    echo "herdr-marketplace: $1" >> "$HERDR_MARKETPLACE_BUILD_LOG" || true
  fi
}

# The compiled dependencies, about 400 MB, are of no use once the binary is
# built: a reinstall replaces the whole checkout. The development build,
# scripts/build.sh on its own, keeps them in target/release.
fallback() {
  report "$1; building from source." >&2
  cleanup
  if [ -f "${HOME:-}/.cargo/env" ]; then
    . "$HOME/.cargo/env"
  fi
  if ! command -v cargo >/dev/null 2>&1; then
    report "cargo not found; install Rust 1.89 from https://rustup.rs to build from source." >&2
    exit 1
  fi
  rm -rf "$source_build"
  status=0
  CARGO_TARGET_DIR="$source_build" sh "$root/scripts/build.sh" || status=$?
  if [ "$status" -eq 0 ]; then
    { mkdir -p "$release_dir" &&
      mv -f "$source_build/release/herdr-marketplace" "$release_dir/herdr-marketplace"; } || status=1
  fi
  rm -rf "$source_build"
  [ "$status" -eq 0 ] && report "built from source; only the binary is kept."
  exit "$status"
}

# curl or GNU wget over HTTPS, following redirects, failing on HTTP errors,
# with bounded delays. BusyBox wget is skipped: it does not verify TLS
# certificates.
download() {
  url="https://github.com/$repo/releases/download/v$version/$1"
  if command -v curl >/dev/null 2>&1; then
    curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
      --connect-timeout 10 --max-time 120 --retry 2 --output "$2" "$url"
  elif command -v wget >/dev/null 2>&1 && wget --version 2>/dev/null | grep -q '^GNU Wget'; then
    wget -q -T 30 -t 2 -O "$2" "$url"
  else
    return 127
  fi
}

fetch() {
  download "$1" "$tmp/$1"
  case $? in
    0) ;;
    127) fallback "neither curl nor GNU wget can download release v$version" ;;
    *) fallback "cannot download $1 of release v$version" ;;
  esac
}

# The tool's exit status is checked before its output is parsed.
sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sum=$(sha256sum "$1") || return 1
  elif command -v shasum >/dev/null 2>&1; then
    sum=$(shasum -a 256 "$1") || return 1
  else
    return 127
  fi
  echo "${sum%% *}"
}

# The SHA-256 that SHA256SUMS gives for $1. Exit status 3: no entry; 4:
# several; 5: malformed.
listed_sum() {
  awk -v name="$1" '
    $NF == name || $NF == "*" name {
      n++
      sum = $1
      ok = length(sum) == 64 && sum ~ /^[0-9a-f]+$/ && ($0 == sum "  " name || $0 == sum " *" name)
    }
    END {
      if (n == 0) exit 3
      if (n > 1) exit 4
      if (!ok) exit 5
      print sum
    }' "$tmp/SHA256SUMS"
}

# README videos play with the FFmpeg of the same release, installed next to
# the binary. Prints why it is not, without building anything.
install_ffmpeg() {
  name="ffmpeg-$triple"
  sum=$(listed_sum "$name")
  case $? in
    0) ;;
    3) echo "SHA256SUMS of release v$version has no entry for $name"; return 1 ;;
    4) echo "SHA256SUMS of release v$version lists $name more than once"; return 1 ;;
    *) echo "SHA256SUMS of release v$version has a malformed entry for $name"; return 1 ;;
  esac
  download "$name" "$tmp/$name" || { echo "cannot download $name of release v$version"; return 1; }
  actual=$(sha256_of "$tmp/$name") || { echo "cannot compute the SHA-256 of $name"; return 1; }
  [ "$actual" = "$sum" ] || { echo "SHA-256 mismatch for $name of release v$version"; return 1; }
  chmod +x "$tmp/$name" || { echo "cannot make $name executable"; return 1; }
  mv -f "$tmp/$name" "$release_dir/ffmpeg" || { echo "cannot install $name in target/release"; return 1; }
}

case "$(uname -s)/$(uname -m)" in
  Darwin/arm64) triple=aarch64-apple-darwin ;;
  Darwin/x86_64) triple=x86_64-apple-darwin ;;
  Linux/x86_64) triple=x86_64-unknown-linux-musl ;;
  *) fallback "no prebuilt binary for $(uname -s)/$(uname -m)" ;;
esac
asset="herdr-marketplace-$triple"

version=$(awk '/^\[/ { pkg = ($0 == "[package]") } pkg && /^version *= *"/ { sub(/^version *= *"/, ""); sub(/".*/, ""); print; exit }' "$root/Cargo.toml")
case $version in
  '' | *[!0-9A-Za-z.+-]*) fallback "cannot read the [package] version in Cargo.toml" ;;
esac

command -v git >/dev/null 2>&1 || fallback "git is required to identify this checkout"
head=$(git -C "$root" rev-parse --verify HEAD 2>/dev/null) || fallback "cannot read the commit of this checkout"
changes=$(git -C "$root" status --porcelain --untracked-files=no 2>/dev/null) || fallback "cannot read the status of this checkout"
[ -z "$changes" ] || fallback "tracked files are modified in this checkout"

# Same filesystem as the installed binary, so the final mv is a rename.
mkdir -p "$release_dir" || fallback "cannot create target/release"
tmp=$(mktemp -d "$release_dir/.herdr-marketplace-fetch.XXXXXX") || fallback "cannot create a temporary directory in target/release"

fetch SOURCE_COMMIT
published=$(awk 'NR == 1 && length($0) == 40 && /^[0-9a-f]+$/ { sha = $0 } END { if (NR == 1 && sha != "") print sha; else exit 1 }' "$tmp/SOURCE_COMMIT") ||
  fallback "SOURCE_COMMIT of release v$version is not a single full commit SHA"
[ "$published" = "$head" ] || fallback "release v$version was built from $published, not from this checkout ($head)"

fetch SHA256SUMS
expected=$(listed_sum "$asset")
case $? in
  0) ;;
  3) fallback "SHA256SUMS of release v$version has no entry for $asset" ;;
  4) fallback "SHA256SUMS of release v$version lists $asset more than once" ;;
  *) fallback "SHA256SUMS of release v$version has a malformed entry for $asset" ;;
esac

fetch "$asset"
actual=$(sha256_of "$tmp/$asset")
case $? in
  0) ;;
  127) fallback "neither sha256sum nor shasum is available" ;;
  *) fallback "cannot compute the SHA-256 of $asset" ;;
esac
[ "$actual" = "$expected" ] || fallback "SHA-256 mismatch for $asset of release v$version"

chmod +x "$tmp/$asset" || fallback "cannot make $asset executable"
mv -f "$tmp/$asset" "$release_dir/herdr-marketplace" || fallback "cannot install $asset in target/release"
# Without FFmpeg, the plugin works all the same.
if reason=$(install_ffmpeg); then
  report "installed verified FFmpeg for README videos."
else
  report "README videos will open in the browser: $reason."
fi
cleanup
report "installed verified prebuilt v$version ($triple) for commit $head."
