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

compile_role_fail_closed_without_selection() {
  local role
  local features
  local log
  local rc
  role=$1
  features=$2
  log="$target_root/$role-missing-selection.log"

  echo "NVCC_ROLE_COMPILE=BEGIN role=$role features=$features arch=sm_120 expected=fail-closed"
  set +e
  env \
    CARGO_TARGET_DIR="$target_root/$role" \
    NOISE_GPU_ARCH=sm_120 \
    NOISE_GPU_DEFINES= \
    PATH="$(dirname "$NVCC"):$PATH" \
    cargo build --release --locked \
      --manifest-path "$ROOT/engine/noise-gpu/Cargo.toml" \
      --no-default-features --features "$features" \
      --bin gpu-surface --bin gpu-airborne >"$log" 2>&1
  rc=$?
  set -e
  cat "$log"
  if [ "$rc" -ne 101 ]; then
    echo "NVCC_ROLE_COMPILE=FAIL role=$role expected_exit=101 actual_exit=$rc" >&2
    return 1
  fi
  if ! grep -Fq \
    'feature `h0-production-selection` requires reviewed `src/h0_production_selection_record.rs` after terminal H0_QUADRATURE_ACCEPTED' \
    "$log"; then
    echo "NVCC_ROLE_COMPILE=FAIL role=$role reason=missing-acceptance-prerequisite-message" >&2
    return 1
  fi
  echo "NVCC_ROLE_COMPILE=FAIL_CLOSED role=$role exit=101 reason=selection-record-absent"
}

"$NVCC" --version
compile_role stock gpu
selection_record="$ROOT/engine/noise-compute/src/h0_production_selection_record.rs"
if [ -L "$selection_record" ]; then
  echo "NVCC_ROLE_MATRIX=FAIL reason=selection-record-is-symlink" >&2
  exit 1
elif [ -e "$selection_record" ]; then
  if [ ! -f "$selection_record" ]; then
    echo "NVCC_ROLE_MATRIX=FAIL reason=selection-record-not-regular" >&2
    exit 1
  fi
  compile_role v2-h0 v2-h0
  selection_state=record-present
else
  compile_role_fail_closed_without_selection v2-h0 v2-h0
  selection_state=record-absent-fail-closed
fi
echo "NVCC_ROLE_MATRIX=PASS roles=stock,v2-h0 arch=sm_120 selection_state=$selection_state cuda_context=not_opened"
