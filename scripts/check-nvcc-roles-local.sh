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
  PATH="$(dirname "$NVCC"):$PATH" \
  cargo build --release --locked \
    --manifest-path "$ROOT/engine/noise-gpu/Cargo.toml" \
    --no-default-features --features gpu \
    --bin gpu-airborne
echo "NVCC_ROLE_COMPILE=PASS role=stock"
# The relevant-source painter: its build script compiles the kernel, generates
# every physics constant from the CPU sources and fails on any .f64 PTX opcode
# (the f64 gate), and its gpu-feature tests run without opening a CUDA context.
# Every cargo command in this focused gate pins NOISE_GPU_ARCH exactly as the
# stock role does: the check remains fast, while the release role builder leaves
# it unset and embeds the FLEET_CUDA_ARCHS SASS image.
echo "NVCC_ROLE_COMPILE=BEGIN role=relevant-source features=gpu arch=sm_120"
env \
  CARGO_TARGET_DIR="$target_root/relevant-source" \
  NOISE_GPU_ARCH=sm_120 \
  PATH="$(dirname "$NVCC"):$PATH" \
  cargo build --release --locked \
    --manifest-path "$ROOT/engine/relevant-source-gpu/Cargo.toml" \
    --features gpu --bin relevant-source-surface
env \
  CARGO_TARGET_DIR="$target_root/relevant-source" \
  NOISE_GPU_ARCH=sm_120 \
  PATH="$(dirname "$NVCC"):$PATH" \
  cargo clippy --release --locked \
    --manifest-path "$ROOT/engine/relevant-source-gpu/Cargo.toml" \
    --features gpu --all-targets -- -D warnings
env \
  CARGO_TARGET_DIR="$target_root/relevant-source" \
  NOISE_GPU_ARCH=sm_120 \
  PATH="$(dirname "$NVCC"):$PATH" \
  cargo test --release --locked \
    --manifest-path "$ROOT/engine/relevant-source-gpu/Cargo.toml" \
    --features gpu
echo "NVCC_ROLE_COMPILE=PASS role=relevant-source f64_gate=passed"
echo "NVCC_ROLE_CHECK=PASS role=stock arch=sm_120 cuda_context=not_opened"
