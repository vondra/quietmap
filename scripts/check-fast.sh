#!/usr/bin/env bash
# check-fast.sh — the public quality gate: frontend, server, pipeline, engine.
# Data-free. The private ops gate extends this with orchestration tests.
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
  (cd server && npm ci --no-audit --no-fund && npx tsc --noEmit -p tsconfig.json && npm test)

  step "pipeline: offline tests"
  (cd pipeline && npm ci --no-audit --no-fund && npm test)

  step "layer topology metadata"
  node scripts/test-layer-spec.mjs

  step "GPU model-role artifacts"
  python3 scripts/test-gpu-model-role.py

  step "shell scripts"
  bash -n scripts/run-extraction.sh scripts/build-heatmap.sh scripts/osm-to-h3r4.sh \
    scripts/run-aircraft-extract.sh scripts/rasters-global.sh \
    scripts/check-surface-stream-concurrency-parity.sh start.sh
fi

if [ "$HALF" != "node" ]; then
  step "engine: GPU define contract"
  "$ROOT/scripts/test-noise-gpu-defines.sh"

  step "engine: rustfmt + clippy + tests"
  for manifest in engine/*/Cargo.toml; do
    cargo fmt --manifest-path "$manifest" -- --check
  done
  for crate in noise-compute source-reader tile-painter; do
    (cd "engine/$crate" && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked --all-targets)
  done

  step "engine: CUDA compile-only role matrix"
  if command -v nvcc >/dev/null 2>&1; then
    "$ROOT/scripts/check-nvcc-roles-local.sh"
  else
    echo "NVCC_ROLE_MATRIX=SKIP reason=nvcc-not-found"
  fi
fi

echo; echo "quality: all OK"
