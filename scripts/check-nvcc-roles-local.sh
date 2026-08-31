#!/usr/bin/env bash
# Compile the production CUDA role without executing a GPU binary.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
NVCC=$(command -v nvcc || true)
if [ -z "$NVCC" ]; then
  echo "NVCC_ROLE_CHECK=FAIL reason=nvcc-not-found" >&2
  exit 1
fi

target_root=$(mktemp -d "${TMPDIR:-/tmp}/quietmap-nvcc-roles.XXXXXX")
cleanup() {
  local original_rc
  original_rc=$1
  trap - EXIT
  case "$target_root" in
    "${TMPDIR:-/tmp}"/quietmap-nvcc-roles.*) rm -rf -- "$target_root" ;;
    *) echo "refusing to remove unexpected target root: $target_root" >&2; original_rc=90 ;;
  esac
  exit "$original_rc"
}
trap 'cleanup $?' EXIT

"$NVCC" --version
echo "NVCC_ROLE_COMPILE=BEGIN role=stock features=gpu arch=sm_120"
env \
  CARGO_TARGET_DIR="$target_root/stock" \
  NOISE_GPU_ARCH=sm_120 \
  NOISE_GPU_DEFINES= \
  PATH="$(dirname "$NVCC"):$PATH" \
  cargo build --release --locked \
    --manifest-path "$ROOT/engine/noise-gpu/Cargo.toml" \
    --no-default-features --features gpu \
    --bin gpu-surface --bin gpu-airborne
echo "NVCC_ROLE_COMPILE=PASS role=stock"
echo "NVCC_ROLE_CHECK=PASS role=stock arch=sm_120 cuda_context=not_opened"
