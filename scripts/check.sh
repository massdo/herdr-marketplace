#!/bin/sh
# Offline checks: format, lint, tests, and the install script with simulated
# downloads. Network tests are marked #[ignore] and run on demand with
# `cargo test -- --ignored`.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
cd "$ROOT"

echo "== fmt =="
cargo fmt --check
echo "== clippy =="
cargo clippy --locked --all-targets -- -D warnings
echo "== test =="
cargo test --locked
sh scripts/test-fetch-or-build.sh
sh scripts/test-release-version.sh
