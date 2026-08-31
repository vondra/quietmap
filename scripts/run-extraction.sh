#!/usr/bin/env bash
# Master extraction script: rasters + OSM in parallel; aircraft/ADS-B starts
# after OSM finishes (Stage 2C needs airport_areas.arrow).
#
# Usage:
#   ADSB_CACHE=/path ./scripts/run-extraction.sh  # all steps; ADS-B cache is required
#   ./scripts/run-extraction.sh rasters      # only rasters
#   ADSB_CACHE=/path ./scripts/run-extraction.sh aircraft
#   ./scripts/run-extraction.sh osm          # only OSM
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR"

LOG_DIR="logs"
mkdir -p "$LOG_DIR"

log() { echo "[main] $(date '+%Y-%m-%d %H:%M:%S') $*"; }
disk_avail() { df -h "$HOME" --output=avail | tail -1 | xargs; }

STEP="${1:-all}"
case "$STEP" in
    all|rasters|aircraft|osm) ;;
    *) echo "Usage: $0 [all|rasters|aircraft|osm]" >&2; exit 1 ;;
esac

# Fail before starting the multi-hour raster/OSM work. The aircraft child
# validates these too, but for `all` that would happen only after OSM finished.
if [ "$STEP" = "all" ] || [ "$STEP" = "aircraft" ]; then
    if [ "${HYBRID:-0}" = "1" ]; then
        [ -n "${AIRLINE_CACHE:-}" ] || { echo "HYBRID=1 requires AIRLINE_CACHE= with an explicit cache directory" >&2; exit 1; }
        [ -n "${GA_CACHE:-}" ] || { echo "HYBRID=1 requires GA_CACHE= with an explicit cache directory" >&2; exit 1; }
    else
        [ -n "${ADSB_CACHE:-}" ] || { echo "requires ADSB_CACHE= with an explicit cache directory" >&2; exit 1; }
    fi
fi

log "========================================"
log "  Extraction: step=$STEP"
log "========================================"
log "  Disk:  $(disk_avail) free"
log "  CPUs:  $(nproc)"
log "  RAM:   $(free -h | awk '/Mem:/{print $2}')"
log ""

T_START=$(date +%s)
PIDS=()
NAMES=()

# ── Rasters (fast, ~5 min) ───────────────────────────────────────────
if [ "$STEP" = "all" ] || [ "$STEP" = "rasters" ]; then
    log "Starting: rasters → $LOG_DIR/extraction-rasters.log"
    bash "$SCRIPT_DIR/rasters-global.sh" convert &> "$LOG_DIR/extraction-rasters.log" &
    PIDS+=($!)
    NAMES+=("rasters")
fi

# ── OSM planet (heavy, ~4-8h) ────────────────────────────────────────
# Must finish before aircraft Stage 2C: that stage reads
# `airport_areas.arrow` per R4 from prepared data for the
# nearest-aerodrome identity lookup, and missing airport geometry
# leaves ground paths without an airport_key. Keep OSM ahead of
# aircraft.
if [ "$STEP" = "all" ] || [ "$STEP" = "osm" ]; then
    log "Starting: osm → $LOG_DIR/extraction-osm.log"
    bash "$SCRIPT_DIR/osm-to-h3r4.sh" &> "$LOG_DIR/extraction-osm.log" &
    OSM_PID=$!
    PIDS+=("$OSM_PID")
    NAMES+=("osm")
fi

# ── Aircraft extract v6 (heavy, ~2-6h) ───────────────────────────────
# Wait for OSM (if running) so airport_*.arrow exists before Stage 2C.
if [ "$STEP" = "all" ] || [ "$STEP" = "aircraft" ]; then
    if [ -n "${OSM_PID:-}" ]; then
        log "Waiting for OSM (pid=$OSM_PID) before launching aircraft → $LOG_DIR/extraction-aircraft.log"
        ( wait "$OSM_PID" 2>/dev/null
          bash "$SCRIPT_DIR/run-aircraft-extract.sh"
        ) &> "$LOG_DIR/extraction-aircraft.log" &
    else
        log "Starting: aircraft → $LOG_DIR/extraction-aircraft.log"
        bash "$SCRIPT_DIR/run-aircraft-extract.sh" &> "$LOG_DIR/extraction-aircraft.log" &
    fi
    PIDS+=($!)
    NAMES+=("aircraft")
fi

if [ ${#PIDS[@]} -eq 0 ]; then
    log "Nothing to do for step=$STEP"
    exit 0
fi

log "${#PIDS[@]} tasks launched"
log ""

# ── Monitor (every 5 min while any task runs) ────────────────────────
(
    while true; do
        sleep 300
        ANY_RUNNING=0
        STATUS=""
        for i in "${!PIDS[@]}"; do
            if kill -0 "${PIDS[$i]}" 2>/dev/null; then
                ANY_RUNNING=1
                STATUS="$STATUS ${NAMES[$i]}:running"
            else
                STATUS="$STATUS ${NAMES[$i]}:done"
            fi
        done
        [ "$ANY_RUNNING" -eq 0 ] && break

        ELAPSED=$(( $(date +%s) - T_START ))
        ELAPSED_HR=$(printf '%dh%02dm' $((ELAPSED/3600)) $(((ELAPSED%3600)/60)))
        log "status ($ELAPSED_HR):$STATUS | disk $(disk_avail)"
    done
) &
MONITOR_PID=$!

# ── Wait ─────────────────────────────────────────────────────────────
FAIL=0
for i in "${!PIDS[@]}"; do
    wait "${PIDS[$i]}"
    RC=$?
    if [ $RC -eq 0 ]; then
        log "${NAMES[$i]}: SUCCESS"
    else
        log "${NAMES[$i]}: FAILED (exit $RC)"
        FAIL=1
    fi
done

kill "$MONITOR_PID" 2>/dev/null || true
wait "$MONITOR_PID" 2>/dev/null || true

# ── Summary ──────────────────────────────────────────────────────────
T_TOTAL=$(( $(date +%s) - T_START ))
ELAPSED_HR=$(printf '%dh%02dm%02ds' $((T_TOTAL/3600)) $(((T_TOTAL%3600)/60)) $((T_TOTAL%60)))

log ""
log "========================================"
log "  EXTRACTION COMPLETE ($ELAPSED_HR)"
log "========================================"

# Count outputs
for dir in data/prepared/2025/h3r4 data/prepared/2026/h3r4; do
    if [ -d "$dir" ]; then
        HEX_COUNT=$(find "$dir" -maxdepth 1 -type d | wc -l)
        SIZE=$(du -sh "$dir" 2>/dev/null | cut -f1)
        log "  $dir: $((HEX_COUNT - 1)) hexes, $SIZE"
    fi
done

for rtype in dem/copernicus dem/srtm rasters/forest rasters/imd; do
    DIR="data/prepared/$rtype"
    if [ -d "$DIR" ]; then
        COUNT=$(ls "$DIR" 2>/dev/null | wc -l)
        log "  $rtype: $COUNT tiles"
    fi
done

log "  Disk free: $(disk_avail)"

exit "$FAIL"
