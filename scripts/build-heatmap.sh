#!/bin/bash
# build-heatmap.sh — orchestrate the whole noise heatmap: build each requested
# layer's z12 tiles + zoom pyramid (z2-11), then the precomputed `total/`
# (energy-sum of every layer, the default all-layers-on view).
#
# Each layer is its own loose staging tree under build/{layer}/ with a distinct
# HM3 source_id; `total/` is derived by build-heatmap-combine. Regenerating one
# layer = rebuild its tree + re-run combine (which re-reads the untouched
# layers). The surface kernels (road/rail/industrial/building) take a bbox or a
# single tile; the aircraft kernels additionally take --world / --shard (their
# region_runner streams the globe through a bounded LRU). So --world here means
# "aircraft world-scale"; surface layers need an explicit bbox or single tile.
#
# Usage (the selection args are forwarded to each builder):
#   ./scripts/build-heatmap.sh --source all --bbox 49.9,14.2,50.2,14.7   # all layers, a region
#   ./scripts/build-heatmap.sh --source road --bbox <s,w,n,e>            # one surface layer + recombine
#   ./scripts/build-heatmap.sh --source all --world                      # aircraft world-scale (surface skipped)
#   ./scripts/build-heatmap.sh --combine-only                            # just rebuild total/ from existing layers
#
# Env: DATA_YEAR=2026  DATA_ROOT=data  OUTPUT=$DATA_ROOT/tiles/$DATA_YEAR/build  ZOOM=12
set -euo pipefail
cd "$(dirname "$0")/.."

DATA_YEAR="${DATA_YEAR:-$(python3 -c 'import json;print(json.load(open("scripts/dataset-year.json"))["current_year"])')}"  # default from committed ./DATA_YEAR (bump there yearly); env overrides
DATA_ROOT="${DATA_ROOT:-data}"
H3R4="$DATA_ROOT/prepared/$DATA_YEAR/h3r4"
PREP="$DATA_ROOT/prepared"
OUTPUT="${OUTPUT:-$DATA_ROOT/tiles/$DATA_YEAR/build}"
# The tile STORE root is its own path, NOT derived from the staging root — the
# 2026-07-09 rename decoupled them (staging=build/, store=store/; /gg Codex).
STORE_ROOT="${STORE_ROOT:-$DATA_ROOT/tiles/$DATA_YEAR/store}"
ZOOM="${ZOOM:-12}"
TARGET=engine/tile-painter/target/release
SURFACE="$TARGET/build-heatmap-surface"
AIRCRAFT="$TARGET/build-heatmap-aircraft"
PYR="$TARGET/build-pyramid"
COMBINE="$TARGET/build-heatmap-combine"
TRANSCODE="$TARGET/tile-store-transcode"
TRANSACTION="$TARGET/tile-store-transaction"
GPU_SURFACE="engine/noise-gpu/target/release/gpu-surface"  # --gpu: line layers on GPU

log() { echo "[build-heatmap] $(date '+%H:%M:%S') $*"; }

# Manual pyramid/combine writes take the SAME _combine flock qm-combine and tile-store-pack
# hold (/gg Codex, 2026-07-15: this path used to run UNLOCKED — a manual full combine could
# TileStore::create-truncate total/ or a pyramid level mid-sidecar-write). BLOCKING acquire:
# a manual run waits out the sidecar's batch (seconds-to-minutes), never skips or races.
# Scoped kernels finish staging before this lock is taken. Their complete ingest → every source
# pyramid → total combine mutation bracket then holds this one master mutex and a durable owned
# root marker. A crash therefore cannot expose a half-updated store to pack/fsck, and an exact
# retry can adopt and finish the marker. Full tile-store-transcode owns the bounded master→ingest
# pair internally through its replacement AND full pyramid. LOCK is computed from the
# CANONICAL stamps dir (combine-loop.sh's key), never from $OUTPUT — bbox runs reassign OUTPUT to a
# run-scoped staging dir and a lock file there would lock nothing anyone else checks.
source "$(dirname "${BASH_SOURCE[0]}")/world/store-lock.sh"
COMBINE_LOCK="$DATA_ROOT/tiles/$DATA_YEAR/build/.stamps/.master._combine.lock"
run_locked() {
  mkdir -p "$(dirname "$COMBINE_LOCK")"
  lock_acquire_wait "$COMBINE_LOCK"
  "$@"; local rc=$?
  lock_release
  return "$rc"
}
# Wall-clock-stamp each line of a long builder's output (same clock as log()), so
# the multi-day surface heartbeat carries "when" — throughput/ETA is read off the
# log. pipefail (set above) still propagates the builder's exit status.
stamp() { while IFS= read -r line; do printf '%s %s\n' "$(date '+%H:%M:%S')" "$line"; done; }

ALL_LAYERS=(road rail industrial building aircraft-airborne aircraft-cruise aircraft-ground)

# Storage redesign 2026-07: kernels stage loose per-cell tiles, then the glue
# into the store depends on the run shape —
#   FULL rebuild : tile-store-transcode (fresh store + exhaustive byte parity + full pyramid;
#                  a green exit licenses deleting the staging tree)
#   SCOPED rebuild: tile-store-ingest   (merge-in-place; every blob decoded +
#                  validated + source_id-checked before it enters the store)
# A bbox/single-tile run stages into a RUN-SCOPED dir so ingest sees exactly this
# run's layers, never leftovers from an older staging.
INGEST="$TARGET/tile-store-ingest"

# Parse --source + orchestration flags; forward everything else (the selection:
# --bbox / --tile-x/--tile-y / --world / --shard) verbatim to the builders.
SOURCE=all
COMBINE_ONLY=false
USE_GPU=false
SEL_ARGS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --source) SOURCE="${2:?--source needs a value}"; shift 2 ;;
    --combine-only) COMBINE_ONLY=true; shift ;;
    --no-combine)
      log "ERROR: --no-combine was removed because it leaves total/ stale; every repaint is one source→pyramid→total transaction"
      exit 2 ;;
    --gpu) USE_GPU=true; shift ;;   # line layers (road/rail) → GPU, point layers ∥ on CPU
    *) SEL_ARGS+=("$1"); shift ;;
  esac
done

case "$SOURCE" in
  all)    LAYERS=("${ALL_LAYERS[@]}") ;;
  ground) LAYERS=(road rail industrial building aircraft-ground) ;;   # the five shared-halo terrain layers
  *)      LAYERS=("$SOURCE") ;;
esac

is_world=false; is_shard=false; is_scoped=false
bbox=""; tile_x=""; tile_y=""; scope_bbox=""
for ((i = 0; i < ${#SEL_ARGS[@]}; i++)); do
  case "${SEL_ARGS[i]}" in
    --world) is_world=true ;;
    --shard) is_shard=true ;;
    --bbox)  bbox="${SEL_ARGS[i + 1]:-}" ;;
    --tile-x) tile_x="${SEL_ARGS[i + 1]:-}" ;;
    --tile-y) tile_y="${SEL_ARGS[i + 1]:-}" ;;
  esac
done

if [ -n "$bbox" ]; then
  is_scoped=true
  if ! scope_bbox=$(python3 - "$bbox" <<'PY'
import math, sys
parts = sys.argv[1].split(',')
if len(parts) != 4:
    raise SystemExit("--bbox needs S,W,N,E")
s, w, n, e = map(float, parts)
if not all(map(math.isfinite, (s, w, n, e))) or not s < n or not w < e:
    raise SystemExit("--bbox needs finite values with S<N and W<E")
print(",".join(format(value, ".17g") for value in (s, w, n, e)))
PY
  ); then
    log "ERROR: invalid --bbox $bbox"
    exit 2
  fi
fi
if [ -n "$tile_x" ] || [ -n "$tile_y" ]; then
  [ -n "$tile_x" ] && [ -n "$tile_y" ] \
    || { log "ERROR: --tile-x and --tile-y must be provided together"; exit 2; }
  [ -z "$bbox" ] || { log "ERROR: --bbox and --tile-x/--tile-y are mutually exclusive"; exit 2; }
  [[ "$tile_x" =~ ^[0-9]+$ && "$tile_y" =~ ^[0-9]+$ ]] \
    || { log "ERROR: tile coordinates must be non-negative integers"; exit 2; }
  tile_limit=$((1 << ZOOM))
  [ "$tile_x" -lt "$tile_limit" ] && [ "$tile_y" -lt "$tile_limit" ] \
    || { log "ERROR: tile coordinates must be below $tile_limit at z$ZOOM"; exit 2; }
  # Store ingest/pyramid accept a bbox, while the kernels also accept one exact tile. Convert that
  # tile to its exact Web-Mercator bounds so silence tombstones and every ancestor are scoped to
  # precisely the same tile rather than promoting a one-tile debug run to a whole-store rebuild.
  scope_bbox=$(python3 - "$ZOOM" "$tile_x" "$tile_y" <<'PY'
import math, sys
z, x, y = map(int, sys.argv[1:])
n = 2 ** z
# `tile_range` treats both bbox edges as inclusive. Move each edge a tiny fraction into the
# tile; exact boundary coordinates would otherwise select and tombstone the east/south neighbours.
inside = 1e-7
west = (x + inside) / n * 360.0 - 180.0
east = (x + 1.0 - inside) / n * 360.0 - 180.0
north = math.degrees(math.atan(math.sinh(math.pi * (1.0 - 2.0 * (y + inside) / n))))
south = math.degrees(math.atan(math.sinh(math.pi * (1.0 - 2.0 * (y + 1.0 - inside) / n))))
print(f"{south:.17g},{west:.17g},{north:.17g},{east:.17g}")
PY
)
  is_scoped=true
fi
if $is_scoped && { $is_world || $is_shard; }; then
  log "ERROR: bbox/tile selectors cannot be combined with --world or --shard"
  exit 2
fi
if $COMBINE_ONLY && { $is_scoped || $is_world || $is_shard; }; then
  log "ERROR: --combine-only takes no repaint selector"
  exit 2
fi
if ! $COMBINE_ONLY && ! $is_scoped && ! $is_world && ! $is_shard; then
  log "ERROR: repaint requires an explicit --bbox, --tile-x/--tile-y, --world, or --shard selector"
  exit 2
fi

# Scoped runs stage into a run-scoped dir (see the glue note above) and the
# staging is deleted after a clean ingest — it is reproducible kernel output.
if $is_scoped; then
  SCOPED_PARENT="$OUTPUT"
  mkdir -p "$SCOPED_PARENT"
  OUTPUT=$(mktemp -d "$SCOPED_PARENT/.scoped-run.XXXXXX")
  # Deleted ONLY on success (end of script) — a failed run keeps its staging
  # for post-mortem. A retry always gets a fresh directory and cannot consume it.
  SCOPED_STAGING="$OUTPUT"
fi

# Rebuild — Fastify dlopen + long jobs cache stale binaries (CLAUDE.md).
log "rebuilding (release)"
cargo build --release --manifest-path engine/tile-painter/Cargo.toml \
  --bin build-heatmap-surface --bin build-heatmap-aircraft \
  --bin build-pyramid --bin build-heatmap-combine --bin tile-store-ingest \
  --bin tile-store-transcode --bin tile-store-transaction

if ! $COMBINE_ONLY; then
  # Split requested layers into the GROUND family (one shared-halo pass) and
  # aircraft (its own region_runner). Ground = road/rail/industrial/building all
  # ray-march terrain, so one 10 km halo per batch feeds every layer.
  SURFACE_LAYERS=(); AIRCRAFT_LAYERS=()
  for L in "${LAYERS[@]}"; do
    case "$L" in
      aircraft-ground) SURFACE_LAYERS+=("$L") ;;   # terrain ray-march → built by the ground pass
      aircraft-*)      AIRCRAFT_LAYERS+=("$L") ;;
      *)               SURFACE_LAYERS+=("$L") ;;
    esac
  done

  # GROUND: build the requested surface layers in ONE process sharing the halo
  # (`--source ground`); a single requested surface layer keeps its own --source
  # for parity. Surface is scoped-only — skipped under --world/--shard.
  if [ ${#SURFACE_LAYERS[@]} -gt 0 ]; then
    if $is_world || $is_shard; then
      log "skip surface (${SURFACE_LAYERS[*]}) — surface kernels are bbox/tile only (use --bbox)"
    elif $USE_GPU; then
      # HYBRID: the line layers (road/rail) go to the GPU while the CPU builds the
      # point/ground layers IN PARALLEL — both mmap the same tmpfs rasters, so the
      # heavy raster reads share the OS page cache (and CPU L3). GPU = throughput
      # on the dominant line scatter; CPU = the cheaper point/ground layers.
      $is_scoped || { log "ERROR: --gpu requires --bbox or --tile-x/--tile-y"; exit 1; }
      # S-3 GPU dense/sparse routing gate: rural road on GPU is a measured 0.82× loss
      # (docs/dev/gpu-benchmark-results.md). Gate by total roads.arrow + railways.arrow
      # bytes over the R4 cells COVERING the bbox (~2 km sampling, the cluster
      # master's polyfill idiom — a whole-tree scan would always read "dense" and
      # walk 121k dirs on the world host). GPU_LINE_MIN_MB default = 2 MB matches
      # the cluster's DENSE_LINE_MB — the same measured dense/sparse boundary.
      # The same R4 set feeds the C9 barrier gate: the GPU kernel has no barrier
      # input, so any barriers.arrow in the bbox routes the line layers to the CPU
      # vector path (mirrors cluster-build-chunk.sh; 11 dB divergence measured on
      # barrier-dense LKPR without it, 2026-06-12). UNLESS QM_GPU_BARRIERS=1: then
      # gpu-surface screens the vector walls itself (kernel ray×segment crossing;
      # spike GO 2026-06-12, 1.97× on barrier-dense vs 1.0× CPU demotion). Default
      # ON (owner-directed 2026-06-13). The "mean 0.002 / max 1.5 dB vs the CPU
      # truth" that used to stand here was measured on the midpoint-projection-and-
      # snap kernel that the 2026-08 exact ray×segment rewrite replaced, so it no
      # longer describes this code and is NOT restated as a bound — the live figure
      # is whatever `e2-full` reports. QM_GPU_BARRIERS=0 forces the CPU demotion.
      GPU_LINE_MIN_MB="${GPU_LINE_MIN_MB:-2}"
      QM_GPU_BARRIERS="${QM_GPU_BARRIERS:-1}"
      bbox_line_bytes=0
      bbox_has_barriers=0
      while read -r r4; do
        [ -d "$H3R4/$r4" ] || continue
        for f in "$H3R4/$r4/roads.arrow" "$H3R4/$r4/railways.arrow"; do
          [ -f "$f" ] && bbox_line_bytes=$((bbox_line_bytes + $(stat -c%s "$f" 2>/dev/null || echo 0)))
        done
        [ -s "$H3R4/$r4/barriers.arrow" ] && bbox_has_barriers=1
      done < <(python3 - "$scope_bbox" <<'PY'
import sys, h3
s, w, n, e = (float(x) for x in sys.argv[1].split(","))
s, n = min(s, n), max(s, n); w, e = min(w, e), max(w, e)
cells = set()
lat = s
while lat <= n + 1e-9:
    lon = w
    while lon <= e + 1e-9:
        cells.add(h3.latlng_to_cell(lat, lon, 4))
        lon += 0.02
    lat += 0.02
# grid_disk(1) union: the builders load RING sources per output cell, so both
# the barrier check (correctness — 1,868 world cells have a neighbour-only
# barrier) and the byte sum (a ring-dense border bbox is NOT sparse) must see
# the ring, not just the covering cells.
ring = set()
for c in cells:
    ring.update(h3.grid_disk(c, 1))
for c in sorted(ring):
    print(c)
PY
)
      # Gate ON: the GPU kernel screens vector barriers, so don't demote on their
      # presence (S-3 sparse demotion below still applies). gpu-surface gets
      # QM_GPU_BARRIERS=1 in its env to actually upload + screen them.
      if [ "$QM_GPU_BARRIERS" = 1 ] && [ "$bbox_has_barriers" -eq 1 ]; then
        log "QM_GPU_BARRIERS=1 — barriers in bbox stay on GPU (kernel screens vector walls)"
        bbox_has_barriers=0
      fi
      gpu_layers=(); cpu_layers=()
      for L in "${SURFACE_LAYERS[@]}"; do
        case "$L" in
          road | rail)
            if [ "$bbox_has_barriers" -eq 1 ] \
              || [ "$bbox_line_bytes" -lt "$((GPU_LINE_MIN_MB * 1000000))" ]; then
              cpu_layers+=("$L")
            else
              gpu_layers+=("$L")
            fi ;;
          *) cpu_layers+=("$L") ;;
        esac
      done
      if [ "${#gpu_layers[@]}" -eq 0 ]; then
        if [ "$bbox_has_barriers" -eq 1 ]; then
          log "barriers in bbox — line layers on CPU (GPU kernel is barrier-blind, C9)"
        else
          log "sparse bbox (${bbox_line_bytes} B < ${GPU_LINE_MIN_MB} MB) — road/rail on CPU (GPU 0.82× rural loss, S-3)"
        fi
      fi
      # --features gpu pulls in cudarc + the nvcc PTX build; build.rs fails with a
      # clear message if nvcc is missing, so this build is the single GPU-host gate.
      log "rebuilding gpu-surface (needs nvcc on this host)"
      cargo build --release --manifest-path engine/noise-gpu/Cargo.toml --features gpu --bin gpu-surface
      gpu_pid=""; cpu_pid=""
      if [ ${#gpu_layers[@]} -gt 0 ]; then
        log "GPU surface: ${gpu_layers[*]} → $OUTPUT/{layer}"
        ( IFS=,; NOISE_GPU_PREPARED="$PREP" DATA_YEAR="$DATA_YEAR" QM_GPU_BARRIERS="$QM_GPU_BARRIERS" \
            scripts/memcap "$GPU_SURFACE" --layers "${gpu_layers[*]}" --bbox "$scope_bbox" --output "$OUTPUT" ) 2>&1 | stamp &
        gpu_pid=$!
      fi
      if [ ${#cpu_layers[@]} -gt 0 ]; then
        # Build exactly cpu_layers via `ground` minus everything not requested.
        excl=(); for L in road rail industrial building aircraft-ground; do
          [[ " ${cpu_layers[*]} " == *" $L "* ]] || excl+=(--exclude "$L")
        done
        log "CPU surface: ${cpu_layers[*]} → $OUTPUT/{layer}"
        scripts/memcap "$SURFACE" --source ground "${excl[@]}" --zoom "$ZOOM" --h3r4-dir "$H3R4" \
          --prepared-dir "$PREP" --output "$OUTPUT" "${SEL_ARGS[@]}" 2>&1 | stamp &
        cpu_pid=$!
      fi
      [ -n "$gpu_pid" ] && wait "$gpu_pid"
      [ -n "$cpu_pid" ] && wait "$cpu_pid"
    else
      # The full surface set (all / ground) → one shared-halo `ground` pass; a
      # single requested layer → its own `--source`. Either way --output is the
      # root and the binary appends {layer}.
      if [ "${#SURFACE_LAYERS[@]}" -ge 2 ]; then SRC=ground; else SRC="${SURFACE_LAYERS[0]}"; fi
      log "build surface $SRC (${SURFACE_LAYERS[*]}) → $OUTPUT/{layer}"
      scripts/memcap "$SURFACE" --source "$SRC" --zoom "$ZOOM" --h3r4-dir "$H3R4" \
        --prepared-dir "$PREP" --output "$OUTPUT" "${SEL_ARGS[@]}" 2>&1 | stamp
    fi
  fi

  # AIRCRAFT: per sub-layer (region_runner streams the globe; --world/--shard ok).
  for L in "${AIRCRAFT_LAYERS[@]}"; do
    LDIR="$OUTPUT/$L"
    if $is_world; then log "clean $LDIR (full rebuild)"; rm -rf "$LDIR"; fi
    log "build $L → $LDIR"
    scripts/memcap "$AIRCRAFT" --source "${L#aircraft-}" --zoom "$ZOOM" --h3r4-dir "$H3R4" \
      --prepared-dir "$PREP" --output "$LDIR" "${SEL_ARGS[@]}"
    if $is_shard; then
      log "sharded — built z$ZOOM only; pyramid $L after merging shards"
    elif $is_scoped; then
      log "staged $L (scoped); store mutation waits for every requested kernel"
    elif $is_world; then
      log "transcode $L → store + pyramid z$ZOOM→z2 (one parity-gated transaction)"
      "$TRANSCODE" "$LDIR" "$STORE_ROOT/$L"
    else
      log "ERROR: aircraft destructive transcode requires explicit --world"
      exit 2
    fi
  done
fi

# Combine into total/ (skip on sharded runs — combine after the merge).
if $is_shard; then
  log "sharded — run combine after merging shards: $COMBINE --store-root <store-root>"
elif $is_scoped; then
  # Recreate these declarations AFTER every kernel: a builder is allowed to replace its output
  # directory. An empty numeric zoom directory means the requested layer was rebuilt and wholly
  # silent in this bbox, so ingest must tombstone its old loud tiles rather than skip the layer.
  for L in "${LAYERS[@]}"; do mkdir -p "$OUTPUT/$L/z$ZOOM"; done
  # The kernels above have all completed. From here until the owned marker is durably removed,
  # one master mutex excludes combine/pack/fsck and the marker makes an interrupted update
  # fail-loud even after this process releases its flock by crashing.
  mkdir -p "$(dirname "$COMBINE_LOCK")"
  lock_acquire_wait "$COMBINE_LOCK"
  descriptor_layers=$(IFS=,; printf '%s' "${LAYERS[*]}")
  "$TRANSACTION" preflight "$STORE_ROOT" "$ZOOM" 2 "$descriptor_layers,total"
  transaction_descriptor="z$ZOOM|bbox=$scope_bbox|layers=$descriptor_layers"
  transaction_token=$("$TRANSACTION" begin "$STORE_ROOT" "$transaction_descriptor")
  [ -n "$transaction_token" ] \
    || { log "ERROR: tile-store-transaction returned an empty owner token"; exit 1; }

  log "ingest all scoped layers → source pyramids → total (one durable transaction)"
  "$INGEST" "$STORE_ROOT" "$OUTPUT" --rebuilt-bbox "$scope_bbox"
  # Ingest owns this lock while it writes. Reacquire it immediately afterwards while the master
  # remains held: a Hub ingest that won the tiny hand-off completes before us and is therefore
  # included; every later ingest waits until all source and total ancestors are coherent.
  exec 8>>"$STORE_ROOT/.ingest.lock"
  flock 8
  for L in "${LAYERS[@]}"; do
    "$PYR" --store-dir "$STORE_ROOT/$L" --base-zoom "$ZOOM" --dst-zoom 2 --bbox "$scope_bbox"
  done
  "$COMBINE" --store-root "$STORE_ROOT" --bbox "$scope_bbox"
  "$TRANSACTION" finish "$STORE_ROOT" "$transaction_token"
  flock -u 8
  exec 8>&-
  lock_release
else
  log "combine → $STORE_ROOT/total"
  run_locked "$COMBINE" --store-root "$STORE_ROOT"
fi
[ -n "${SCOPED_STAGING:-}" ] && rm -rf "$SCOPED_STAGING"
log "done → $STORE_ROOT (pack + publish: tile-store-pack <store-root> <pmtiles-dir> b<N>)"
