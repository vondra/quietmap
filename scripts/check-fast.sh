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

  step "pipeline: typecheck + offline tests"
  (cd pipeline && npm ci --no-audit --no-fund && npm run typecheck && npm test)

  step "layer topology metadata"
  node scripts/test-layer-spec.mjs

  step "GPU model-role artifacts"
  python3 scripts/test-gpu-model-role.py

  step "shell scripts"
  bash -n scripts/run-extraction.sh scripts/build-heatmap.sh scripts/osm-to-h3r4.sh \
    scripts/run-aircraft-extract.sh scripts/rasters-global.sh \
    start.sh scripts/rasters/convert-forest-continuous.sh
fi

if [ "$HALF" != "node" ]; then
  step "engine: GPU define contract"
  "$ROOT/scripts/test-noise-gpu-defines.sh"

  step "engine: rustfmt + clippy + tests"
  cargo fmt --manifest-path engine/Cargo.toml --all -- --check
  for crate in noise-compute source-reader tile-painter; do
    (cd engine && cargo clippy --locked -p "$crate" --all-targets -- -D warnings \
      && cargo test --locked -p "$crate" --all-targets)
  done

  # The ground hoist must stay bit-exact under release optimisation; the debug
  # all-targets run above cannot detect compiler/libm constant-folding drift.
  step "engine: optimized ground-hoist exactness"
  (cd engine && cargo test --locked --release -p noise-compute --test ground_hoist_exact)

  step "engine: CUDA compile-only production role"
  if command -v nvcc >/dev/null 2>&1; then
    "$ROOT/scripts/check-nvcc-roles-local.sh"
  else
    echo "NVCC_ROLE_CHECK=SKIP reason=nvcc-not-found"
  fi
fi

echo; echo "quality: all OK"
