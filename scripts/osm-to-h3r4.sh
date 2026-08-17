#!/usr/bin/env bash
# Extract noise-relevant features from OSM planet PBF → H3R4 Arrow IPC.
#
# Output: data/prepared/{year}/h3r4/{hex}/ with roads.arrow, railways.arrow, buildings.arrow, industrial.arrow
#
# Usage: ./scripts/osm-to-h3r4.sh   (all knobs are env vars — see below; PBF_FILE must exist)
# Called by run-extraction.sh (osm step); also run standalone for an OSM refresh.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR"

YEAR="${DATA_YEAR:-$(python3 -c 'import json;print(json.load(open("scripts/dataset-year.json"))["current_year"])')}"  # default from committed ./DATA_YEAR (bump there yearly); env overrides
DATA_ROOT="${DATA_ROOT:-$PROJECT_DIR/data}"
RUN_SERVICE_TREE="${RUN_SERVICE_TREE:-1}"
STAMP_ROAD_METADATA="${STAMP_ROAD_METADATA:-1}"
if [[ -d /mnt/data ]]; then
    DEFAULT_SCRATCH_ROOT="/mnt/data/tmp/quietmap"
else
    DEFAULT_SCRATCH_ROOT="/tmp/quietmap"
fi
SCRATCH_ROOT="${SCRATCH_ROOT:-$DEFAULT_SCRATCH_ROOT}"
PBF_FILE="${PBF_FILE:-$DATA_ROOT/source/osm/${YEAR}/planet-latest.osm.pbf}"
OUTPUT_DIR="${OUTPUT_DIR:-$DATA_ROOT/prepared/${YEAR}/h3r4}"
NODE_CACHE="${NODE_CACHE:-$SCRATCH_ROOT/osm_nodes.cache}"
SPILL_DIR="${SPILL_DIR:-$SCRATCH_ROOT/osm_spill}"
BINARY="engine/osm-extract/target/release/osm-to-h3r4"
ROAD_ARROW_UPGRADE_BIN="engine/source-reader/target/release/road-arrow-upgrade"

log() { echo "[osm] $(date '+%H:%M:%S') $*"; }

if [ ! -f "$BINARY" ]; then
    log "ERROR: Binary not found: $BINARY"
    exit 1
fi

if [ ! -f "$PBF_FILE" ]; then
    log "ERROR: Planet PBF not found: $PBF_FILE"
    exit 1
fi

PBF_SIZE=$(stat --printf='%s' "$PBF_FILE")
PBF_SIZE_HR=$(numfmt --to=iec-i --suffix=B "$PBF_SIZE")

mkdir -p "$OUTPUT_DIR" "$(dirname "$NODE_CACHE")" "$SPILL_DIR"

log "=== OSM extraction ==="
log "  Input:      $PBF_FILE ($PBF_SIZE_HR)"
log "  Output:     $OUTPUT_DIR"
log "  Scratch:    $SCRATCH_ROOT"
log "  Node cache: $NODE_CACHE"
log "  Spill dir:  $SPILL_DIR"
log "  Disk free:  output $(df -h "$OUTPUT_DIR" --output=avail | tail -1 | xargs) | scratch $(df -h "$SCRATCH_ROOT" --output=avail | tail -1 | xargs)"

T_START=$(date +%s)

# Background monitor: report progress every 2 min
(
    while true; do
        sleep 120
        NOW=$(date +%s)
        ELAPSED=$((NOW - T_START))
        ELAPSED_HR=$(printf '%dh%02dm' $((ELAPSED/3600)) $(((ELAPSED%3600)/60)))
        HEX_COUNT=$(find "$OUTPUT_DIR" -maxdepth 1 -type d 2>/dev/null | wc -l)
        CACHE_SIZE=0
        [ -f "$NODE_CACHE" ] && CACHE_SIZE=$(stat --printf='%s' "$NODE_CACHE" 2>/dev/null || echo 0)
        CACHE_HR=$(numfmt --to=iec-i --suffix=B "$CACHE_SIZE" 2>/dev/null || echo "?")
        DISK_FREE_OUTPUT=$(df -h "$OUTPUT_DIR" --output=avail | tail -1 | xargs)
        DISK_FREE_SCRATCH=$(df -h "$SCRATCH_ROOT" --output=avail | tail -1 | xargs)
        log "  progress: $ELAPSED_HR | hexes $HEX_COUNT | node-cache $CACHE_HR | output $DISK_FREE_OUTPUT | scratch $DISK_FREE_SCRATCH"
    done
) &
MONITOR_PID=$!

"$BINARY" \
    --input "$PBF_FILE" \
    --output "$OUTPUT_DIR" \
    --node-cache "$NODE_CACHE" \
    --spill-dir "$SPILL_DIR" \
    2>&1 | while IFS= read -r line; do log "  $line"; done

kill "$MONITOR_PID" 2>/dev/null || true
wait "$MONITOR_PID" 2>/dev/null || true

T_ELAPSED=$(( $(date +%s) - T_START ))
HEX_COUNT=$(find "$OUTPUT_DIR" -maxdepth 1 -type d 2>/dev/null | wc -l)
OUTPUT_SIZE=$(du -sh "$OUTPUT_DIR" 2>/dev/null | cut -f1)

log ""
# Reclaim scratch the moment the binary is done with it: the node cache (~100 GB)
# AND the sort spill (~190 GB) are useless after finalize. Leaving the spill behind
# stranded ~250 GB of dead scratch per run on /tmp (the qm-reextract leak, fixed
# 2026-06-25). Output arrows are already written to OUTPUT_DIR, not scratch.
log "Cleaning up scratch (node cache + spill) ..."
rm -f "$NODE_CACHE"
rm -rf "$SPILL_DIR"

# Admin table BEFORE the road passes: service-tree resolves each hex's country
# for the national vehicle-mix/trip-rate tables, and readAdminIso() on a missing
# file silently returns an empty map — running it after (the pre-2026-07 order)
# made every fresh extract fall back to WORLD defaults unnoticed (/gg Codex
# CRITICAL, trip-generators plan). Needs only the hex dir listing + Natural
# Earth polygons, so right after extraction is the earliest correct slot.
log ""
log "Building H3R4 → admin lookup table (data/prepared/h3r4-admin.bin) ..."
(
    cd "$SCRIPT_DIR"
    if [ ! -d node_modules ]; then
        npm ci 2>&1 | while IFS= read -r line; do log "    $line"; done
    fi
    DATA_YEAR="$YEAR" npm run build:h3-admin
) 2>&1 | while IFS= read -r line; do log "  $line"; done

if [ "$RUN_SERVICE_TREE" = "1" ]; then
    if [ ! -d "$PROJECT_DIR/pipeline/node_modules" ]; then
        log ""
        log "Installing pipeline dependencies ..."
        npm --prefix "$PROJECT_DIR/pipeline" ci \
            2>&1 | while IFS= read -r line; do log "  $line"; done
    fi

    log ""
    SVC_SHARDS="${SVC_SHARDS:-96}"
    SVC_JOBS="${SVC_JOBS:-$(nproc)}"
    SVC_LOG_DIR="$SCRATCH_ROOT/svc-tree-logs"
    BUP_LOG_DIR="$SCRATCH_ROOT/built-up-logs"
    CTRY_LOG_DIR="$SCRATCH_ROOT/country-bake-logs"
    export SVC_SHARDS SVC_LOG_DIR BUP_LOG_DIR CTRY_LOG_DIR YEAR

    # built_up flag (urban/rural from the building raster, task #15) BEFORE
    # service-tree — order between the two is irrelevant for correctness (this
    # pass only writes the built_up column), it just belongs with the other
    # fresh-extract road passes. Same per-hex SHARD parallelism as service-tree.
    log "Running built-up road flagging ($SVC_SHARDS shards x $SVC_JOBS jobs) ..."
    rm -rf "$BUP_LOG_DIR"; mkdir -p "$BUP_LOG_DIR"
    (
        cd "$PROJECT_DIR/pipeline"
        seq 0 $((SVC_SHARDS - 1)) | xargs -P "$SVC_JOBS" -I{} bash -c \
            'SHARD="$1/$SVC_SHARDS" DATA_YEAR="$YEAR" node_modules/.bin/tsx enrich-roads-built-up.ts > "$BUP_LOG_DIR/shard-$1.log" 2>&1' _ {}
    ) || true
    # Same completion gate as service-tree: a shard counts only if it reached
    # "=== Results" — catches OOM-kills / missing tsx, not just error strings.
    set +e
    bup_done=$(grep -lF "=== Results" "$BUP_LOG_DIR"/shard-*.log 2>/dev/null | wc -l)
    bup_stats=$(grep -h "segments:" "$BUP_LOG_DIR"/shard-*.log 2>/dev/null | \
        awk '{rows+=$1; urban+=$3; rural+=$6; unk+=$9} END {printf "%d segments (%d urban / %d rural / %d unknown)", rows, urban, rural, unk}')
    set -e
    log "  built-up: ${bup_stats:-no output}, $bup_done/$SVC_SHARDS shards completed"
    if [ "$bup_done" -ne "$SVC_SHARDS" ]; then
        # FATAL, not a warning: a partial built_up pass silently degrades untagged-road
        # speeds for whole regions (engine reads absent/0 as "use legacy") — re-running
        # this script is idempotent, so failing hard costs nothing (/gg Codex).
        log "  FATAL: $((SVC_SHARDS - bup_done)) built-up shard(s) did NOT complete (died/OOM/missing-binary) — see $BUP_LOG_DIR"
        exit 1
    fi

    # Acquire the CGAZ ADM0 dataset ONCE before fan-out: border hexes make the
    # shards read the geoBoundaries GeoJSON, and a fresh host would run the same
    # download/convert 96× concurrently against the same .tmp files (/gg diff
    # review). This is the ONLY step allowed to download it — the gate library
    # itself only reads, and fails loud when the dataset is missing, so a shard
    # can never quietly proceed ungated. Single-process here, cache hits in shards.
    log ""
    log "Preparing CGAZ country-polygon caches ..."
    (
        cd "$PROJECT_DIR/pipeline"
        node_modules/.bin/tsx prepare-country-polygon-caches.ts
    ) 2>&1 | while IFS= read -r line; do log "  $line"; done

    # Per-segment country/city/continent bake (plan M3) AFTER the CGAZ warm —
    # AdminAt needs the caches. Writes country_iso/city_id/continent into
    # roads.arrow AND railways.arrow with the country_baked_v1 contract stamp;
    # independent of built-up/service-tree (different columns). Same per-hex
    # SHARD parallelism; idempotent byte-identical re-runs.
    log ""
    log "Running per-segment country bake ($SVC_SHARDS shards x $SVC_JOBS jobs) ..."
    rm -rf "$CTRY_LOG_DIR"; mkdir -p "$CTRY_LOG_DIR"
    (
        cd "$PROJECT_DIR/pipeline"
        seq 0 $((SVC_SHARDS - 1)) | xargs -P "$SVC_JOBS" -I{} bash -c \
            'SHARD="$1/$SVC_SHARDS" DATA_YEAR="$YEAR" node_modules/.bin/tsx enrich-roads-country.ts > "$CTRY_LOG_DIR/shard-$1.log" 2>&1' _ {}
    ) || true
    # Same completion gate as built-up/service-tree ("=== Results" = the shard
    # finished) PLUS a FATAL-line check: the bake exits nonzero when a hex's
    # on-land rows resolve 100% UNKNOWN (the G1 trap — resolver silently dead
    # for that hex), and a baked UNKNOWN world must never ship silently.
    set +e
    ctry_done=$(grep -lF "=== Results" "$CTRY_LOG_DIR"/shard-*.log 2>/dev/null | wc -l)
    ctry_fatal=$(grep -lF "FATAL:" "$CTRY_LOG_DIR"/shard-*.log 2>/dev/null | wc -l)
    ctry_stats=$(grep -h "^  roads: " "$CTRY_LOG_DIR"/shard-*.log 2>/dev/null | \
        awk '{for(i=2;i<=NF;i++){split($i,a,"=");s[a[1]]+=a[2]}} END {printf "rows=%d resolved=%d unknown_on_land=%d disputed=%d offshore=%d metro=%d", s["rows"], s["resolved"], s["unknown_on_land"], s["disputed"], s["offshore"], s["metro"]}')
    set -e
    log "  country-bake roads: ${ctry_stats:-no output}, $ctry_done/$SVC_SHARDS shards completed"
    if [ "$ctry_done" -ne "$SVC_SHARDS" ] || [ "$ctry_fatal" -gt 0 ]; then
        # FATAL, not a warning (same reasoning as built-up above): partial or
        # 100%-UNKNOWN bakes silently degrade per-segment admin resolution for
        # whole regions — re-running this script is idempotent, so failing hard
        # costs nothing.
        log "  FATAL: $((SVC_SHARDS - ctry_done)) country-bake shard(s) did NOT complete and $ctry_fatal shard(s) hit the on-land-UNKNOWN invariant — see $CTRY_LOG_DIR"
        exit 1
    fi

    log ""
    log "Running service-tree road enrichment ($SVC_SHARDS shards x $SVC_JOBS jobs) ..."
    rm -rf "$SVC_LOG_DIR"; mkdir -p "$SVC_LOG_DIR"
    # Per-hex flow accumulation is single-threaded, but every hex is self-contained
    # (reads/writes only its own roads.arrow), so SHARD=i/n slices run in parallel —
    # ~3h single-core -> ~15min. Local tsx, NOT npx: parallel `npx tsx` races on the
    # shared ~/.npm/_npx cache and most shards die with ENOENT.
    (
        cd "$PROJECT_DIR/pipeline"
        seq 0 $((SVC_SHARDS - 1)) | xargs -P "$SVC_JOBS" -I{} bash -c \
            'SHARD="$1/$SVC_SHARDS" DATA_YEAR="$YEAR" node_modules/.bin/tsx enrich-roads-service-tree.ts > "$SVC_LOG_DIR/shard-$1.log" 2>&1' _ {}
    ) || true
    # A shard "completed" iff it reached the final "=== Results" line. Counting that
    # (not error-string greps) catches OOM-kills, a missing tsx binary, or any crash —
    # otherwise a died shard leaves its hex slice un-enriched (and maybe a half-written
    # roads.arrow) and the extract would march on to stamping unaware.
    set +e
    svc_done=$(grep -lF "=== Results" "$SVC_LOG_DIR"/shard-*.log 2>/dev/null | wc -l)
    svc_enr=$(grep -h "Segments:" "$SVC_LOG_DIR"/shard-*.log 2>/dev/null | grep -oE "[0-9]+ enriched" | grep -oE "^[0-9]+" | awk '{s+=$1} END {print s+0}')
    set -e
    log "  service-tree: $svc_enr segments enriched, $svc_done/$SVC_SHARDS shards completed"
    if [ "$svc_done" -ne "$SVC_SHARDS" ]; then
        # FATAL like built-up above: a died shard leaves its hex slice with
        # wrong traffic (fresh extract: zeroes) and the extract would march on
        # to stamping unaware (/gg diff review). Re-running is idempotent.
        log "  FATAL: $((SVC_SHARDS - svc_done)) service-tree shard(s) did NOT complete (died/OOM/missing-binary) — see $SVC_LOG_DIR"
        exit 1
    fi

    if [ "$STAMP_ROAD_METADATA" = "1" ]; then
        if [ ! -f "$ROAD_ARROW_UPGRADE_BIN" ]; then
            log ""
            log "Building road-arrow-upgrade ..."
            cargo build --release --manifest-path engine/source-reader/Cargo.toml --bin road-arrow-upgrade \
                2>&1 | while IFS= read -r line; do log "  $line"; done
        fi

        log ""
        log "Stamping road Arrow provenance ..."
        "$ROAD_ARROW_UPGRADE_BIN" "$OUTPUT_DIR" \
            2>&1 | while IFS= read -r line; do log "  $line"; done
    fi
fi

log ""
log "=== OSM extraction DONE ==="
log "  $HEX_COUNT hex directories, $OUTPUT_SIZE total"
log "  Time: $(printf '%dh%02dm%02ds' $((T_ELAPSED/3600)) $(((T_ELAPSED%3600)/60)) $((T_ELAPSED%60)))"
log "  Disk free: output $(df -h "$OUTPUT_DIR" --output=avail | tail -1 | xargs) | scratch $(df -h "$SCRATCH_ROOT" --output=avail | tail -1 | xargs)"
