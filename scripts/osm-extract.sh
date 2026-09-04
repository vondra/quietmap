#!/usr/bin/env bash
# Extract noise-relevant features from OSM planet PBF → z9 Arrow IPC.
#
# Output: $OUTPUT_DIR/z9/<x>/<y>/{roads,railways,buildings,industrial,
#   barriers,leisure,airport_areas,airport_lines}.arrow (integer grid geometry)
#
# Required env: PBF_FILE (planet), OUTPUT_DIR (the release prepared dir).
# Optional: SCRATCH_ROOT (node cache + spill, ~300 GB during the run),
#   NUM_BUCKETS (spill partitions).
# Enrichment (structures, service-tree, country bake, …) runs as separate
# steps after this; see the pipeline transfers.
set -euo pipefail

: "${PBF_FILE:?set PBF_FILE to the planet .osm.pbf}"
: "${OUTPUT_DIR:?set OUTPUT_DIR to the release prepared dir}"
SCRATCH_ROOT="${SCRATCH_ROOT:-/data/mixeduse2/scratch}"
NUM_BUCKETS="${NUM_BUCKETS:-256}"

NODE_CACHE="$SCRATCH_ROOT/osm_nodes.cache"
SPILL_DIR="$SCRATCH_ROOT/osm_spill"
BINARY="$(cd "$(dirname "$0")/.." && pwd)/engine/target/release/osm-extract"

log() { echo "[osm] $(date '+%H:%M:%S') $*"; }

if [ ! -f "$BINARY" ]; then
    log "building osm-extract ..."
    cargo build --release --manifest-path "$(dirname "$BINARY")/../Cargo.toml"
fi
if [ ! -f "$PBF_FILE" ]; then
    log "ERROR: Planet PBF not found: $PBF_FILE"
    exit 1
fi

PBF_SIZE_HR=$(numfmt --to=iec-i --suffix=B "$(stat --printf='%s' "$PBF_FILE")")
mkdir -p "$OUTPUT_DIR" "$(dirname "$NODE_CACHE")" "$SPILL_DIR"

log "=== OSM extraction ==="
log "  Input:      $PBF_FILE ($PBF_SIZE_HR)"
log "  Output:     $OUTPUT_DIR"
log "  Scratch:    $SCRATCH_ROOT"
log "  Disk free:  output $(df -h "$OUTPUT_DIR" --output=avail | tail -1 | xargs) | scratch $(df -h "$SCRATCH_ROOT" --output=avail | tail -1 | xargs)"

T_START=$(date +%s)

# Background monitor: report progress every 2 min
(
    while true; do
        sleep 120
        NOW=$(date +%s)
        ELAPSED=$((NOW - T_START))
        ELAPSED_HR=$(printf '%dh%02dm' $((ELAPSED/3600)) $(((ELAPSED%3600)/60)))
        SQ_COUNT=$(find "$OUTPUT_DIR/z9" -maxdepth 2 -mindepth 2 -type d 2>/dev/null | wc -l)
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

kill "$MONITOR_PID" 2>/dev/null || true
wait "$MONITOR_PID" 2>/dev/null || true

# Reclaim scratch the moment the binary is done with it: the node cache and
# the sort spill are useless after finalize.
log "Cleaning up scratch (node cache + spill) ..."
rm -f "$NODE_CACHE"
rm -rf "$SPILL_DIR"

T_ELAPSED=$(( $(date +%s) - T_START ))
SQ_COUNT=$(find "$OUTPUT_DIR/z9" -maxdepth 2 -mindepth 2 -type d 2>/dev/null | wc -l)
OUTPUT_SIZE=$(du -sh "$OUTPUT_DIR" 2>/dev/null | cut -f1)

log ""
log "=== OSM extraction DONE ==="
log "  $SQ_COUNT square directories, $OUTPUT_SIZE total"
log "  Time: $(printf '%dh%02dm%02ds' $((T_ELAPSED/3600)) $(((T_ELAPSED%3600)/60)) $((T_ELAPSED%60)))"
