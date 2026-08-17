#!/usr/bin/env bash
# check-renderer-evidence-output-parity.sh — prove the evidence flag cannot alter HM3/PTX bytes.
set -euo pipefail

if [ "$#" -ne 5 ]; then
  echo "usage: $0 ROLE BINARY PREPARED_ROOT CELLS_FILE SCRATCH_ROOT" >&2
  echo "roles: cpu-rest cpu-cruise gpu-line gpu-airborne" >&2
  exit 64
fi

role=$1
binary=$(realpath "$2")
prepared=$(realpath "$3")
cells=$(realpath "$4")
scratch=$5
h3r4="$prepared/2026/h3r4"
fixture_commit=2222222222222222222222222222222222222222
fixture_model_sha=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
fixture_layer_sha=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb

test -x "$binary" || { echo "not executable: $binary" >&2; exit 65; }
test -d "$h3r4" || { echo "missing prepared H3R4 directory: $h3r4" >&2; exit 66; }
test -s "$cells" || { echo "empty cells file: $cells" >&2; exit 67; }
test ! -e "$scratch" || { echo "scratch root must be absent: $scratch" >&2; exit 68; }
case "$role" in
  cpu-rest|cpu-cruise) ;;
  gpu-line|gpu-airborne)
    test "${QM_RENDERER_PARITY_GPU_AUTHORITY:-}" = YES || {
      echo "GPU parity requires an external HOLD and QM_RENDERER_PARITY_GPU_AUTHORITY=YES" >&2
      exit 69
    }
    ;;
  *) echo "unknown role: $role" >&2; exit 64 ;;
esac
mkdir -p "$scratch"
sha256sum "$binary" >"$scratch/binary-before.sha256"

run_arm() {
  local batch=$1 evidence=$2
  local arm="batch${batch}-${evidence}"
  local output="$scratch/output-$arm"
  local stdout="$scratch/$arm.stdout"
  local stderr="$scratch/$arm.stderr"
  local -a common_env=(
    HOME="$HOME" PATH="$PATH" LD_LIBRARY_PATH="${LD_LIBRARY_PATH:-}"
    DATA_YEAR=2026 RAYON_NUM_THREADS=32 QM_GPU_STREAM_WORKERS=2
    QM_VECTOR_BUILDINGS=1
  )
  local -a evidence_env=()
  if [ "$evidence" = on ]; then
    evidence_env=(
      QM_RENDERER_EVIDENCE_V1=1
      QM_RENDERER_LANE="$role"
      QM_RENDERER_PRODUCT_COMMIT="$fixture_commit"
      QM_RENDERER_ARTIFACT_FAMILY="parity-fixture-$role"
      QM_RENDERER_LINE_MODEL_ROLE_SHA256="$fixture_model_sha"
      QM_RENDERER_LAYER_SPEC_SHA256="$fixture_layer_sha"
    )
  fi
  case "$role" in
    cpu-rest)
      env -i "${common_env[@]}" "${evidence_env[@]}" "$binary" \
        --stream --source ground --exclude road --exclude rail \
        --seed-regions "$cells" --h3r4-dir "$h3r4" --prepared-dir "$prepared" \
        --zoom 13 --batch-size "$batch" --n-days 12 --region-concurrency 2 \
        --output "$output" <"$cells" >"$stdout" 2>"$stderr"
      ;;
    cpu-cruise)
      env -i "${common_env[@]}" "${evidence_env[@]}" "$binary" \
        --stream --source cruise --seed-regions "$cells" --h3r4-dir "$h3r4" \
        --prepared-dir "$prepared" --zoom 13 --batch-size "$batch" --n-days 12 \
        --output "$output" <"$cells" >"$stdout" 2>"$stderr"
      ;;
    gpu-line)
      env -i "${common_env[@]}" "${evidence_env[@]}" \
        NOISE_GPU_PREPARED="$prepared" QM_GPU_BARRIERS=1 "$binary" \
        --stream --zoom 13 --layers road,rail --batch "$batch" --output "$output" \
        <"$cells" >"$stdout" 2>"$stderr"
      ;;
    gpu-airborne)
      env -i "${common_env[@]}" "${evidence_env[@]}" "$binary" \
        --stream --seed-regions "$cells" --h3r4-dir "$h3r4" \
        --prepared-dir "$prepared" --zoom 13 --batch-size "$batch" --n-days 12 \
        --output "$output" <"$cells" >"$stdout" 2>"$stderr"
      ;;
  esac
  test "$(grep -c '^fail ' "$stdout" || true)" -eq 0
  test "$(grep -c '^done ' "$stdout" || true)" -eq "$(wc -l <"$cells")"
  if [ "$evidence" = on ]; then
    test "$(grep -c '^{' "$stdout" || true)" -gt 0
    test "$(grep -c '"schema":"runtime-shape-v1"' "$stdout" || true)" -eq 1
  else
    test "$(grep -c '"schema":"runtime-shape-v1"' "$stdout" || true)" -eq 0
  fi
  (
    cd "$output"
    find . -type f -name '*.bin' -printf '%P\0' | sort -z | xargs -0 -r sha256sum
  ) >"$scratch/$arm-output.sha256"
  test -s "$scratch/$arm-output.sha256" || {
    echo "parity arm produced no HM3 payloads: $arm" >&2
    return 70
  }
}

for batch in 2 4; do
  run_arm "$batch" off
  run_arm "$batch" on
  cmp "$scratch/batch${batch}-off-output.sha256" "$scratch/batch${batch}-on-output.sha256"
done
sha256sum "$binary" >"$scratch/binary-after.sha256"
cmp "$scratch/binary-before.sha256" "$scratch/binary-after.sha256"
printf 'RENDERER_EVIDENCE_OUTPUT_PARITY=PASS role=%s batches=2,4 embedded_ptx_container_unchanged=YES\n' "$role"
