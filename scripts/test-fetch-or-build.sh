#!/bin/sh
# Offline tests for scripts/fetch-or-build.sh, adapted from herdr-npm v0.1.0.
# Each case copies the script into a throwaway git checkout and runs it with
# `env -i` on an isolated PATH: uname, curl, wget and cargo are simulated;
# chmod, mv and the SHA-256 tool can be made to fail. No network access and no
# real compilation.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
trap 'exit 1' HUP INT TERM

VERSION=9.8.7
TRIPLES="aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-musl"
if command -v sha256sum >/dev/null 2>&1; then SHA_TOOL=sha256sum; else SHA_TOOL=shasum; fi

hash_files() {
  if [ "$SHA_TOOL" = sha256sum ]; then sha256sum "$@"; else shasum -a 256 "$@"; fi
}

fixture_git() {
  HOME="$WORK" XDG_CONFIG_HOME="$WORK" GIT_CONFIG_NOSYSTEM=1 git -C "$P" \
    -c init.defaultBranch=main -c user.name=test -c user.email=test@example.invalid \
    -c commit.gpgsign=false "$@"
}

# Real tools: always present, or switchable per case.
mkdir -p "$WORK/base" "$WORK/real" "$WORK/fakes"
for tool in sh awk cat cp dirname find grep head ls mkdir mktemp rm; do
  ln -s "$(command -v "$tool")" "$WORK/base/$tool"
done
for tool in chmod mv git "$SHA_TOOL"; do
  ln -s "$(command -v "$tool")" "$WORK/real/$tool"
done

cat > "$WORK/fakes/uname" <<'EOF'
#!/bin/sh
case ${1:-} in
  -m) echo "$FAKE_ARCH" ;;
  *) echo "$FAKE_OS" ;;
esac
EOF

# curl and wget: serve $FAKE_RELEASE/v<version>/<name>. FAKE_FAIL holds
# "<name>:<exit status>" or "<name>:partial" rules; a missing file is a 404.
cat > "$WORK/fakes/download" <<'EOF'
#!/bin/sh
tool=${0##*/}
if [ "$tool" = wget ] && [ "${1:-}" = --version ]; then
  echo "$FAKE_WGET_VERSION"
  exit 0
fi
echo "$tool $*" >> "$FAKE_LOG/downloads"
out="" url=""
while [ $# -gt 0 ]; do
  case $1 in
    --output | -O) out=$2; shift ;;
    https://*) url=$1 ;;
  esac
  shift
done
prefix=https://github.com/massdo/herdr-marketplace/releases/download/
case $url in
  "$prefix"*) path=${url#"$prefix"} ;;
  *) echo "unexpected URL: $url" >&2; exit 90 ;;
esac
name=${path##*/}
for rule in $FAKE_FAIL; do
  case $rule in
    "$name:partial") head -c 10 "$FAKE_RELEASE/$path" > "$out"; exit 18 ;;
    "$name:"*) exit "${rule#*:}" ;;
  esac
done
[ -f "$FAKE_RELEASE/$path" ] || exit 22
cp "$FAKE_RELEASE/$path" "$out"
EOF

# Records the source build instead of compiling, with a compiled dependency
# next to the binary, as cargo leaves them.
cat > "$WORK/fakes/cargo" <<'EOF'
#!/bin/sh
echo "$*" > "$FAKE_LOG/cargo.args"
echo "${CARGO_TARGET_DIR:-}" > "$FAKE_LOG/cargo.target"
find target/release -name '.herdr-marketplace-fetch.*' > "$FAKE_LOG/cargo.leftovers" 2>/dev/null
out=${CARGO_TARGET_DIR:-target}/release
mkdir -p "$out/deps"
echo dependency > "$out/deps/libdependency.rlib"
echo compiled > "$out/herdr-marketplace"
exit "$FAKE_CARGO_EXIT"
EOF

cat > "$WORK/fakes/broken" <<'EOF'
#!/bin/sh
echo "${0##*/}: simulated failure" >&2
exit 1
EOF
chmod +x "$WORK"/fakes/*

cases=0
failures=0

# A clean checkout whose commit is published in a complete release, on
# Darwin/arm64 with curl, git, the SHA-256 tool and cargo on PATH.
setup() {
  NAME=$1
  cases=$((cases + 1))
  failures_before=$failures
  C="$WORK/case$cases"
  P="$C/plugin" B="$C/bin" L="$C/log" R="$C/release/v$VERSION"
  mkdir -p "$P/scripts" "$B" "$L" "$R" "$C/home"
  cp "$ROOT/scripts/fetch-or-build.sh" "$ROOT/scripts/build.sh" "$P/scripts/"
  # The decoy version before [package] must not be used.
  cat > "$P/Cargo.toml" <<EOF
[workspace.package]
version = "0.0.0"

[package]
name = "herdr-marketplace"
version = "$VERSION"
EOF
  fixture_git init -q
  fixture_git add .
  fixture_git commit -q -m fixture
  HEAD_SHA=$(fixture_git rev-parse HEAD)
  echo "$HEAD_SHA" > "$R/SOURCE_COMMIT"
  for triple in $TRIPLES; do
    printf '#!/bin/sh\necho %s > "%s/executed"\n' "$triple" "$L" > "$R/herdr-marketplace-$triple"
  done
  (cd "$R" && hash_files herdr-marketplace-*) > "$R/SHA256SUMS"
  for tool in "$WORK"/base/* "$WORK"/real/*; do
    ln -s "$tool" "$B/"
  done
  ln -s "$WORK/fakes/uname" "$B/uname"
  ln -s "$WORK/fakes/download" "$B/curl"
  ln -s "$WORK/fakes/cargo" "$B/cargo"
  OS=Darwin ARCH=arm64 FAIL="" WGET_VERSION="GNU Wget 1.21.4" CARGO_EXIT=0
  BUILD_LOG="$L/build.log"
}

break_tool() {
  rm -f "$B/$1"
  ln -s "$WORK/fakes/broken" "$B/$1"
}

drop_sums_entry() {
  grep -v " herdr-marketplace-$1\$" "$R/SHA256SUMS" > "$C/SHA256SUMS" || true
  cp "$C/SHA256SUMS" "$R/SHA256SUMS"
}

run() {
  if env -i HOME="$C/home" PATH="$B" HERDR_MARKETPLACE_BUILD_LOG="$BUILD_LOG" FAKE_LOG="$L" \
    FAKE_RELEASE="$C/release" FAKE_OS="$OS" FAKE_ARCH="$ARCH" FAKE_FAIL="$FAIL" \
    FAKE_WGET_VERSION="$WGET_VERSION" FAKE_CARGO_EXIT="$CARGO_EXIT" \
    "$B/sh" "$P/scripts/fetch-or-build.sh" > "$L/stdout" 2> "$L/stderr"; then
    status=0
  else
    status=$?
  fi
}

fail() {
  echo "not ok - $NAME: $1" >&2
  sed 's/^/    stderr: /' "$L/stderr" >&2
  failures=$((failures + 1))
}

done_case() {
  if [ -d "$P/target/release" ] && [ -n "$(find "$P/target/release" -name '.herdr-marketplace-fetch.*')" ]; then
    fail "a temporary directory is left in target/release"
  fi
  [ ! -e "$L/executed" ] || fail "a downloaded binary was executed"
  if [ "$failures" -eq "$failures_before" ]; then
    echo "ok - $NAME"
  fi
}

expect_prebuilt() {
  [ "$status" -eq 0 ] || fail "exit status $status, expected 0"
  cmp -s "$R/herdr-marketplace-$1" "$P/target/release/herdr-marketplace" || fail "herdr-marketplace-$1 is not installed"
  [ -x "$P/target/release/herdr-marketplace" ] || fail "the installed binary is not executable"
  grep -qF "installed verified prebuilt v$VERSION ($1) for commit $HEAD_SHA." "$L/stdout" || fail "no success message"
  grep -qF "installed verified prebuilt v$VERSION ($1) for commit $HEAD_SHA." "$L/build.log" || fail "no success message in HERDR_MARKETPLACE_BUILD_LOG"
  [ ! -s "$L/stderr" ] || fail "unexpected stderr"
  [ ! -e "$L/cargo.args" ] || fail "cargo ran although the binary was verified"
  done_case
}

# $1: reason on stderr; $2: expected exit status. A source build happens in
# target/source-build, of which only the binary is kept.
expect_source_build() {
  [ "$status" -eq "${2:-0}" ] || fail "exit status $status, expected ${2:-0}"
  grep -qF "herdr-marketplace: $1; building from source." "$L/stderr" || fail "stderr lacks: $1"
  grep -qF "herdr-marketplace: $1; building from source." "$L/build.log" || fail "HERDR_MARKETPLACE_BUILD_LOG lacks: $1"
  [ "$(cat "$L/cargo.args" 2>/dev/null)" = "build --release --locked" ] || fail "cargo was not run as build --release --locked"
  case $(cat "$L/cargo.target" 2>/dev/null) in
    */target/source-build) ;;
    *) fail "cargo did not build in target/source-build" ;;
  esac
  [ ! -s "$L/cargo.leftovers" ] || fail "temporary files remained when cargo started"
  [ ! -e "$P/target/source-build" ] || fail "target/source-build remains after the build"
  if [ "${2:-0}" -eq 0 ]; then
    [ "$(cat "$P/target/release/herdr-marketplace" 2>/dev/null)" = compiled ] || fail "the installed binary is not the source build"
    grep -qF "built from source; only the binary is kept." "$L/stdout" || fail "no message about the source build"
  else
    [ ! -e "$P/target/release/herdr-marketplace" ] || fail "a binary was installed although the build failed"
  fi
  if grep -q 'installed verified prebuilt' "$L/stdout"; then fail "announced a prebuilt binary"; fi
  done_case
}

echo "== fetch-or-build (offline, $SHA_TOOL) =="

for triple in $TRIPLES; do
  setup "prebuilt $triple with curl"
  case $triple in
    aarch64-apple-darwin) OS=Darwin ARCH=arm64 ;;
    x86_64-apple-darwin) OS=Darwin ARCH=x86_64 ;;
    x86_64-unknown-linux-musl) OS=Linux ARCH=x86_64 ;;
  esac
  run
  for flag in --fail --location "--proto-redir =https" "--connect-timeout 10" "--max-time 120"; do
    grep -qF -- "$flag" "$L/downloads" || fail "curl ran without $flag"
  done
  expect_prebuilt "$triple"
done

setup "prebuilt with GNU wget when curl is absent"
rm "$B/curl"
ln -s "$WORK/fakes/download" "$B/wget"
OS=Linux ARCH=x86_64
run
expect_prebuilt x86_64-unknown-linux-musl

setup "unwritable HERDR_MARKETPLACE_BUILD_LOG"
BUILD_LOG="$C/missing/build.log"
run
[ "$status" -eq 0 ] || fail "exit status $status, expected 0"
cmp -s "$R/herdr-marketplace-aarch64-apple-darwin" "$P/target/release/herdr-marketplace" || fail "herdr-marketplace-aarch64-apple-darwin is not installed"
done_case

setup "release absent"
rm -r "$R"
run
expect_source_build "cannot download SOURCE_COMMIT of release v$VERSION"

setup "HTTP error on the binary"
FAIL="herdr-marketplace-aarch64-apple-darwin:22"
run
expect_source_build "cannot download herdr-marketplace-aarch64-apple-darwin of release v$VERSION"

setup "timeout on SHA256SUMS"
FAIL="SHA256SUMS:28"
run
expect_source_build "cannot download SHA256SUMS of release v$VERSION"

setup "partial transfer of the binary"
FAIL="herdr-marketplace-aarch64-apple-darwin:partial"
run
expect_source_build "cannot download herdr-marketplace-aarch64-apple-darwin of release v$VERSION"

setup "wrong checksum"
echo tampered >> "$R/herdr-marketplace-aarch64-apple-darwin"
run
expect_source_build "SHA-256 mismatch for herdr-marketplace-aarch64-apple-darwin of release v$VERSION"

setup "checksum absent"
drop_sums_entry aarch64-apple-darwin
run
expect_source_build "SHA256SUMS of release v$VERSION has no entry for herdr-marketplace-aarch64-apple-darwin"

setup "malformed checksum"
drop_sums_entry aarch64-apple-darwin
echo "0123456789abcdef  herdr-marketplace-aarch64-apple-darwin" >> "$R/SHA256SUMS"
run
expect_source_build "SHA256SUMS of release v$VERSION has a malformed entry for herdr-marketplace-aarch64-apple-darwin"

setup "duplicated checksum"
entry=$(grep " herdr-marketplace-aarch64-apple-darwin\$" "$R/SHA256SUMS")
echo "$entry" >> "$R/SHA256SUMS"
run
expect_source_build "SHA256SUMS of release v$VERSION lists herdr-marketplace-aarch64-apple-darwin more than once"

setup "SHA-256 tool absent"
rm "$B/$SHA_TOOL"
run
expect_source_build "neither sha256sum nor shasum is available"

setup "SHA-256 tool failing"
break_tool "$SHA_TOOL"
run
expect_source_build "cannot compute the SHA-256 of herdr-marketplace-aarch64-apple-darwin"

setup "release built from another commit"
echo 0123456789abcdef0123456789abcdef01234567 > "$R/SOURCE_COMMIT"
run
expect_source_build "release v$VERSION was built from 0123456789abcdef0123456789abcdef01234567, not from this checkout ($HEAD_SHA)"

setup "invalid SOURCE_COMMIT"
echo "$HEAD_SHA" >> "$R/SOURCE_COMMIT"
run
expect_source_build "SOURCE_COMMIT of release v$VERSION is not a single full commit SHA"

setup "git absent"
rm "$B/git"
run
expect_source_build "git is required to identify this checkout"

setup "modified checkout"
echo "# local change" >> "$P/Cargo.toml"
run
expect_source_build "tracked files are modified in this checkout"

setup "unknown platform"
OS=Linux ARCH=aarch64
run
expect_source_build "no prebuilt binary for Linux/aarch64"

setup "no download tool"
rm "$B/curl"
run
expect_source_build "neither curl nor GNU wget can download release v$VERSION"

setup "BusyBox wget only"
rm "$B/curl"
ln -s "$WORK/fakes/download" "$B/wget"
WGET_VERSION="BusyBox v1.36.1"
run
expect_source_build "neither curl nor GNU wget can download release v$VERSION"

setup "chmod failing"
break_tool chmod
run
expect_source_build "cannot make herdr-marketplace-aarch64-apple-darwin executable"

# The source build installs its binary with mv too, so it fails as well.
setup "mv failing"
break_tool mv
run
expect_source_build "cannot install herdr-marketplace-aarch64-apple-darwin in target/release" 1

setup "compilation failing"
rm -r "$R"
CARGO_EXIT=101
run
expect_source_build "cannot download SOURCE_COMMIT of release v$VERSION" 101

setup "cargo only through \$HOME/.cargo/env"
rm "$B/cargo"
rm -r "$R"
mkdir -p "$C/home/.cargo" "$C/cargo-bin"
ln -s "$WORK/fakes/cargo" "$C/cargo-bin/cargo"
echo "export PATH=\"$C/cargo-bin:\$PATH\"" > "$C/home/.cargo/env"
run
expect_source_build "cannot download SOURCE_COMMIT of release v$VERSION"

setup "cargo absent, old binary in place"
rm "$B/cargo"
rm -r "$R"
mkdir -p "$P/target/release"
echo old > "$P/target/release/herdr-marketplace"
run
[ "$status" -ne 0 ] || fail "succeeded without cargo"
grep -qF "cannot download SOURCE_COMMIT of release v$VERSION; building from source." "$L/stderr" || fail "no reason on stderr"
grep -qF "cargo not found" "$L/stderr" || fail "no message about the missing cargo"
grep -qF "cargo not found" "$L/build.log" || fail "no message about the missing cargo in HERDR_MARKETPLACE_BUILD_LOG"
[ "$(cat "$P/target/release/herdr-marketplace")" = old ] || fail "the old binary was replaced"
if grep -q 'installed verified prebuilt' "$L/stdout"; then fail "announced a prebuilt binary"; fi
done_case

echo "fetch-or-build: $cases cases, $failures failures"
[ "$failures" -eq 0 ]
