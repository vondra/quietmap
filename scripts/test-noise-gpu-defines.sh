#!/usr/bin/env bash
# Prove that only reviewed CUDA experiment defines reach nvcc.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
BUILD_RS="$ROOT/engine/noise-gpu/build.rs"
work_root=$(mktemp -d "${TMPDIR:-/tmp}/quietmap-gpu-defines.XXXXXX")

cleanup() {
  local original_rc=$1
  trap - EXIT
  case "$work_root" in
    "${TMPDIR:-/tmp}"/quietmap-gpu-defines.*) rm -rf -- "$work_root" ;;
    *) echo "refusing to remove unexpected test root: $work_root" >&2; original_rc=90 ;;
  esac
  exit "$original_rc"
}
trap 'cleanup $?' EXIT

rustc --edition=2021 --test "$BUILD_RS" -o "$work_root/build-rs-tests"
"$work_root/build-rs-tests"

rustc --edition=2021 "$BUILD_RS" -o "$work_root/build-rs"
mkdir -p "$work_root/bin" "$work_root/out"
cat >"$work_root/bin/nvcc" <<'FAKE_NVCC'
#!/usr/bin/env bash
set -euo pipefail
: >"$NVCC_INVOCATION_MARKER"
exit 97
FAKE_NVCC
chmod +x "$work_root/bin/nvcc"

assert_rejected_before_nvcc() {
  local mutation=$1
  local label=$2
  local log="$work_root/$label.log"
  local marker="$work_root/$label.nvcc-invoked"
  local out="$work_root/out/$label"
  local rc=0

  rm -f -- "$marker"
  mkdir -p "$out"
  (
    cd "$ROOT/engine/noise-gpu"
    env \
      CARGO_FEATURE_GPU=1 \
      NOISE_GPU_DEFINES="$mutation" \
      OUT_DIR="$out" \
      NVCC_INVOCATION_MARKER="$marker" \
      PATH="$work_root/bin:$PATH" \
      "$work_root/build-rs"
  ) >"$log" 2>&1 || rc=$?

  if [ "$rc" -eq 0 ]; then
    echo "GPU_DEFINE_MUTATION=FAIL label=$label reason=unexpected-success" >&2
    return 1
  fi
  if [ -e "$marker" ]; then
    echo "GPU_DEFINE_MUTATION=FAIL label=$label reason=reached-nvcc" >&2
    return 1
  fi
  if ! grep -Fq 'invalid NOISE_GPU_DEFINES:' "$log"; then
    echo "GPU_DEFINE_MUTATION=FAIL label=$label reason=wrong-failure" >&2
    cat "$log" >&2
    return 1
  fi
  echo "GPU_DEFINE_MUTATION=PASS label=$label rejected_before_nvcc=1"
}

assert_define_set_reaches_nvcc() {
  local defines=$1
  local label=$2
  local log="$work_root/$label.log"
  local marker="$work_root/$label.nvcc-invoked"
  local out="$work_root/out/$label"
  local rc=0

  mkdir -p "$out"
  (
    cd "$ROOT/engine/noise-gpu"
    env \
      CARGO_FEATURE_GPU=1 \
      NOISE_GPU_DEFINES="$defines" \
      OUT_DIR="$out" \
      NVCC_INVOCATION_MARKER="$marker" \
      PATH="$work_root/bin:$PATH" \
      "$work_root/build-rs"
  ) >"$log" 2>&1 || rc=$?

  if [ "$rc" -eq 0 ] || [ ! -e "$marker" ]; then
    echo "GPU_DEFINE_ALLOWLIST=FAIL label=$label reason=accepted-set-did-not-reach-nvcc rc=$rc" >&2
    cat "$log" >&2
    return 1
  fi
  if grep -Fq 'invalid NOISE_GPU_DEFINES:' "$log"; then
    echo "GPU_DEFINE_ALLOWLIST=FAIL label=$label reason=accepted-set-rejected" >&2
    cat "$log" >&2
    return 1
  fi
  if [ ! -s "$out/nvcc-defines.txt" ]; then
    echo "GPU_DEFINE_ALLOWLIST=FAIL label=$label reason=missing-define-receipt" >&2
    return 1
  fi
  echo "GPU_DEFINE_ALLOWLIST=PASS label=$label reached_nvcc=1"
}

assert_rejected_before_nvcc '-DBARRIER_STRIDE=1' barrier-stride
assert_rejected_before_nvcc '-DSOURCE_SEGMENT_STRIDE=1' segment-stride
assert_rejected_before_nvcc '-DLINE_KERNEL_ARGUMENT_COUNT=1' argument-count
assert_rejected_before_nvcc '-DSURFACE_META_SLOTS=1' surface-meta-slots
assert_rejected_before_nvcc '-DM_LAT=1' latitude-scale
assert_rejected_before_nvcc '-DOUT_ARCSTAT_COUNTERS=1' output-layout
assert_rejected_before_nvcc '-DMULTIFIDELITY_COMPACT_ABI_VERSION=1' compact-abi
assert_rejected_before_nvcc '-DTPX=1' architecture
assert_rejected_before_nvcc '-DBIN_W=1' bin-width
assert_rejected_before_nvcc '-DNPD_NC=1' npd-classes
assert_rejected_before_nvcc '-DUNKNOWN_SWITCH=1' unknown
assert_rejected_before_nvcc '-UARC_TRI_WALK' undefine
assert_rejected_before_nvcc '@defines.rsp' response-file
assert_rejected_before_nvcc '--compiler-options=-DTPX=1' compiler-option
assert_rejected_before_nvcc '-D ARC_TRI_WALK=0' split-define
assert_rejected_before_nvcc '-DARC_TRI_WALK=0 -DARC_TRI_WALK=1' duplicate
assert_define_set_reaches_nvcc '' production-empty
assert_define_set_reaches_nvcc '-DARC_TRI_WALK=0 -DPROF_COUNTERS=1' reviewed-experiment
if grep -Eq '^-D(ARC_TRI_WALK|PROF_COUNTERS)(=|$)' \
  "$work_root/out/production-empty/nvcc-defines.txt"; then
  echo "GPU_DEFINE_RECEIPT=FAIL reason=production-receipt-contains-experiment" >&2
  exit 1
fi
grep -Fxq -- '-DARC_TRI_WALK=0' "$work_root/out/reviewed-experiment/nvcc-defines.txt"
grep -Fxq -- '-DPROF_COUNTERS=1' "$work_root/out/reviewed-experiment/nvcc-defines.txt"
PYTHONPATH="$ROOT/scripts" python3 - \
  "$work_root/out/production-empty/nvcc-defines.txt" \
  "$work_root/out/reviewed-experiment/nvcc-defines.txt" <<'PY'
import sys
from pathlib import Path

from gpu_model_role import parse_nvcc_define_receipt

_, production_experimental = parse_nvcc_define_receipt(Path(sys.argv[1]))
_, reviewed_experimental = parse_nvcc_define_receipt(Path(sys.argv[2]))
if production_experimental:
    raise SystemExit("production define receipt contains an experimental macro")
if reviewed_experimental != ["-DARC_TRI_WALK=0", "-DPROF_COUNTERS=1"]:
    raise SystemExit(f"reviewed define receipt mismatch: {reviewed_experimental}")
print("GPU_DEFINE_VERIFIER=PASS production_experimental=0 reviewed_experimental=2")
PY
echo "GPU_DEFINE_RECEIPT=PASS production_empty=1 reviewed_experiment=2"

echo "GPU_DEFINE_MUTATIONS=PASS mutations=16 rejected_nvcc_invocations=0"
