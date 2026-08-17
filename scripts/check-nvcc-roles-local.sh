#!/usr/bin/env bash
# Compile both production CUDA feature roles without executing a GPU binary.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
NVCC=$(command -v nvcc || true)
if [ -z "$NVCC" ]; then
  echo "NVCC_ROLE_MATRIX=FAIL reason=nvcc-not-found" >&2
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

compile_role() {
  local role=$1
  local features=$2

  echo "NVCC_ROLE_COMPILE=BEGIN role=$role features=$features arch=sm_120"
  env \
    CARGO_TARGET_DIR="$target_root/$role" \
    NOISE_GPU_ARCH=sm_120 \
    NOISE_GPU_DEFINES= \
    PATH="$(dirname "$NVCC"):$PATH" \
    cargo build --release --locked \
      --manifest-path "$ROOT/engine/noise-gpu/Cargo.toml" \
      --no-default-features --features "$features" \
      --bin gpu-surface --bin gpu-airborne
  echo "NVCC_ROLE_COMPILE=PASS role=$role"
}

"$NVCC" --version
compile_role stock gpu
compile_role v2-h0 v2-h0
echo "NVCC_ROLE_MATRIX=PASS roles=stock,v2-h0 arch=sm_120 cuda_context=not_opened"
