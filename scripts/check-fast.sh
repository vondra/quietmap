#!/usr/bin/env bash
# check-fast.sh — the data-free quality gate: frontend, server, pipeline,
# scripts (Python unittest + shell syntax) and the engine workspace.
# Optional first arg: `node` (skip Rust) or `rust` (skip Node), for CI split jobs.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd -P)
cd "$ROOT"
HALF="${1:-all}"

step() { echo; echo "== $*"; }

if [ "$HALF" != "rust" ]; then
  step "frontend: lint + build + unit tests"
  (cd frontend && npm ci --no-audit --no-fund && npm run lint && npm run build && npm test)

  step "server: typecheck + tests"
  (cd server && npm ci --no-audit --no-fund && npm run typecheck && npm test)

  step "pipeline: typecheck + offline tests"
  (cd pipeline && npm ci --no-audit --no-fund && npm run typecheck && npm test)

  # Each test module imports its neighbours by bare name, so it runs from its
  # own directory; discover does not descend into these package-less folders.
  step "scripts: Python unittest modules, each from its own directory"
  for directory in scripts scripts/rasters scripts/roads scripts/square-country-city \
      scripts/structures scripts/overture; do
    echo "-- $directory"
    (cd "$directory" && python3 -m unittest discover -s . -p 'test_*.py')
  done

  step "scripts: shell syntax"
  bash -n scripts/*.sh scripts/*/*.sh
fi

if [ "$HALF" != "node" ]; then
  # rustfmt is not part of the gate: engine files predate it (14 differ, 2026-09-11).
  step "engine: clippy (release, all targets) + workspace tests"
  (cd engine && cargo clippy --locked --release --workspace --all-targets -- -D warnings)
  (cd engine && cargo test --locked --workspace)

  # The N-API surface is off by default; its lints and tests need the feature.
  # The `gpu` feature stays off: it needs nvcc and is not a hosted-runner gate.
  step "engine: source-reader node feature (clippy + tests)"
  (cd engine && cargo clippy --locked --release -p source-reader --features node \
      --all-targets -- -D warnings)
  (cd engine && cargo test --locked -p source-reader --features node)
fi

echo; echo "quality: all OK"
