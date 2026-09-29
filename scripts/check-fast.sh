#!/usr/bin/env bash
# The repository gate: formatting, lints and tests of every part that exists. Zero warnings.
set -euo pipefail
cd "$(dirname "$0")/.."

git diff --check
git diff --cached --check
python3 -m json.tool bench/points.json > /dev/null

if [ -f Cargo.toml ]; then
  cargo fmt --all -- --check
  cargo clippy --workspace --all-targets --release -- -D warnings
  cargo test --workspace --release --quiet
fi

if [ -f frontend/package.json ]; then
  npm --prefix frontend run --silent check
fi

echo "check-fast: ok"
