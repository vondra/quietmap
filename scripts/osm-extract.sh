#!/usr/bin/env bash
# Extract noise-relevant features from OSM planet PBF → z9 Arrow IPC.
#
# Output: $OUTPUT_DIR/z9/<x>/<y>/{roads,railways,buildings,industrial,
#   barriers,leisure,airport_areas,airport_lines}.arrow (integer grid geometry)
#
# Required env: PBF_FILE (planet), OUTPUT_DIR (the release prepared dir).
# Optional scratch (defaults keep both under SCRATCH_ROOT; override to put
# the cache and spill on different disks — do not guess a striping layout):
#   SCRATCH_ROOT  parent of the default cache + spill (~300 GB during the run)
#   NODE_CACHE    Pass 1 sparse mmap write, then Pass 2 random coordinate reads
#   SPILL_DIR     Pass 2 sequential feature writes and finalize sort temp
#   NUM_BUCKETS   spill partitions
# A complete spill left in SPILL_DIR by a failed finalize is finalized again
# without reading the planet (the binary decides; the planet stays a pinned
# input of the step). Cleanup deletes only NODE_CACHE and SPILL_DIR, never
# PBF_FILE or other source trees.
# Enrichment (structures, service-tree, country bake, …) runs as separate
# steps after this; see the pipeline transfers.
set -euo pipefail

log() { echo "[osm] $(date '+%H:%M:%S') $*"; }

: "${PBF_FILE:?set PBF_FILE to the planet .osm.pbf}"
: "${OUTPUT_DIR:?set OUTPUT_DIR to the release prepared dir}"
SCRATCH_ROOT="${SCRATCH_ROOT:-${TMPDIR:-/tmp}/quietmap-osm}"
NODE_CACHE="${NODE_CACHE:-$SCRATCH_ROOT/osm_nodes.cache}"
SPILL_DIR="${SPILL_DIR:-$SCRATCH_ROOT/osm_spill}"
NUM_BUCKETS="${NUM_BUCKETS:-256}"
if ! [[ "$NUM_BUCKETS" =~ ^[1-9][0-9]*$ ]]; then
    log "ERROR: NUM_BUCKETS must be a positive integer, got: $NUM_BUCKETS"
    exit 1
fi

# Nine spill streams keep one writer per bucket open. Raise only the soft
# limit to the derived requirement; changing the process hard limit is neither
# necessary nor guaranteed to be permitted.
REQUIRED_OPEN_FILES=$((9 * NUM_BUCKETS + 64))
HARD_OPEN_FILES=$(ulimit -Hn)
if [ "$HARD_OPEN_FILES" != "unlimited" ] && [ "$REQUIRED_OPEN_FILES" -gt "$HARD_OPEN_FILES" ]; then
    log "ERROR: extraction needs $REQUIRED_OPEN_FILES open files for $NUM_BUCKETS buckets; hard limit is $HARD_OPEN_FILES"
    exit 1
fi
SOFT_OPEN_FILES=$(ulimit -Sn)
if [ "$SOFT_OPEN_FILES" != "unlimited" ] && [ "$SOFT_OPEN_FILES" -lt "$REQUIRED_OPEN_FILES" ]; then
    ulimit -Sn "$REQUIRED_OPEN_FILES"
fi

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BINARY="$REPO_ROOT/engine/target/release/osm-extract"

MONITOR_PID=""
stop_monitor() {
    if [ -n "$MONITOR_PID" ]; then
        kill "$MONITOR_PID" 2>/dev/null || true
        wait "$MONITOR_PID" 2>/dev/null || true
        MONITOR_PID=""
    fi
}
trap stop_monitor EXIT

log "building osm-extract (incremental) ..."
cargo build --release --manifest-path "$REPO_ROOT/engine/Cargo.toml" --bin osm-extract
if [ ! -f "$PBF_FILE" ]; then
    log "ERROR: Planet PBF not found: $PBF_FILE"
    exit 1
fi

PBF_SIZE_HR=$(numfmt --to=iec-i --suffix=B "$(stat --printf='%s' "$PBF_FILE")")

log "=== OSM extraction ==="
log "  Input:      $PBF_FILE ($PBF_SIZE_HR)"
log "  Output:     $OUTPUT_DIR"
log "  Node cache: $NODE_CACHE  (Pass 1 write, Pass 2 random reads)"
log "  Spill:      $SPILL_DIR  (Pass 2 writes, finalize sort)"

T_START=$(date +%s)

# Background monitor: report progress every 2 min
(
    while true; do
        sleep 120
        NOW=$(date +%s)
        ELAPSED=$((NOW - T_START))
        ELAPSED_HR=$(printf '%dh%02dm' $((ELAPSED/3600)) $(((ELAPSED%3600)/60)))
        SQ_COUNT=$(find "$OUTPUT_DIR/z9" -maxdepth 2 -mindepth 2 -type d 2>/dev/null | wc -l) || SQ_COUNT=0
        CACHE_SIZE=0
        [ -f "$NODE_CACHE" ] && CACHE_SIZE=$(stat --printf='%s' "$NODE_CACHE" 2>/dev/null || echo 0)
        CACHE_HR=$(numfmt --to=iec-i --suffix=B "$CACHE_SIZE" 2>/dev/null || echo "?")
        log "  progress: $ELAPSED_HR | squares $SQ_COUNT | node-cache $CACHE_HR"
    done
) &
MONITOR_PID=$!

"$BINARY" \
    --input "$PBF_FILE" \
    --output "$OUTPUT_DIR" \
    --node-cache "$NODE_CACHE" \
    --spill-dir "$SPILL_DIR" \
    --num-buckets "$NUM_BUCKETS" \
    2>&1 | while IFS= read -r line; do log "  $line"; done

stop_monitor

T_ELAPSED=$(( $(date +%s) - T_START ))
SQ_COUNT=$(find "$OUTPUT_DIR/z9" -maxdepth 2 -mindepth 2 -type d 2>/dev/null | wc -l) || SQ_COUNT=0
OUTPUT_SIZE=$(du -sh "$OUTPUT_DIR" 2>/dev/null | cut -f1)

log ""
log "=== OSM extraction DONE ==="
log "  $SQ_COUNT square directories, $OUTPUT_SIZE total"
log "  Time: $(printf '%dh%02dm%02ds' $((T_ELAPSED/3600)) $(((T_ELAPSED%3600)/60)) $((T_ELAPSED%60)))"
