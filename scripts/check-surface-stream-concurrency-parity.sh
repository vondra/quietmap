#!/usr/bin/env bash
# check-surface-stream-concurrency-parity.sh — compare canonical CPU-surface HM3 bytes at N=1/2.
set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "usage: $0 BUILD_HEATMAP_SURFACE PREPARED_ROOT SCRATCH_ROOT" >&2
  exit 64
fi

binary=$(realpath "$1")
prepared=$(realpath "$2")
scratch=$3
h3r4="$prepared/2026/h3r4"

test -x "$binary" || { echo "not executable: $binary" >&2; exit 65; }
test -d "$h3r4" || { echo "missing prepared H3R4 directory: $h3r4" >&2; exit 66; }
test ! -e "$scratch" || { echo "scratch root must be absent: $scratch" >&2; exit 67; }
mkdir -p "$scratch"

cells="$scratch/cells.txt"
printf '%s\n' 841e309ffffffff 841e355ffffffff >"$cells"

run_arm() {
  local concurrency=$1
  local output="$scratch/output-n$concurrency"
  local stdout="$scratch/n$concurrency.stdout"
  local stderr="$scratch/n$concurrency.stderr"
  env -i \
    HOME="$HOME" PATH="$PATH" RAYON_NUM_THREADS=32 DATA_YEAR=2026 \
    "$binary" --stream --source ground --exclude road --exclude rail \
    --seed-regions "$cells" --h3r4-dir "$h3r4" --prepared-dir "$prepared" \
    --zoom 13 --batch-size 2 --n-days 12 --region-concurrency "$concurrency" \
    --output "$output" <"$cells" >"$stdout" 2>"$stderr"
  test "$(grep -c '^done ' "$stdout")" -eq 2
  test "$(grep -c '^fail ' "$stdout")" -eq 0
  test "$(grep -c 'max_regions_per_claim=1' "$stderr")" -eq 1
  (
    cd "$output"
    find . -type f -name '*.bin' -printf '%P\0' | sort -z | xargs -0 -r sha256sum
  ) >"$scratch/n$concurrency-output.sha256"
  test -s "$scratch/n$concurrency-output.sha256"
}

run_arm 1
run_arm 2
cmp "$scratch/n1-output.sha256" "$scratch/n2-output.sha256"
printf 'SURFACE_STREAM_CONCURRENCY_PARITY=PASS\n'
