#!/usr/bin/env bash
# Global raster pipeline: download + convert all raster data.
#
# Steps (parallelized where possible):
#   1. Download Copernicus GLO-30 DEM + GHSL (parallel)
#   2. Convert SRTM + WorldCover (parallel)
#   3. Convert Copernicus DEM (after download)
#   4. IMD overlay (after WorldCover)
#
# Usage:
#   ./scripts/rasters-global.sh           # full pipeline
#   ./scripts/rasters-global.sh convert   # skip downloads, only convert
#
# Called by run-extraction.sh (rasters step, `convert` mode); sub-steps live in scripts/rasters/.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR"

LOG_DIR="logs"
mkdir -p "$LOG_DIR"

log() { echo "[rasters] $(date '+%H:%M:%S') $*"; }

MODE="${1:-all}"
T_START=$(date +%s)

log "=== Global raster pipeline (mode=$MODE) ==="
log "  Disk free: $(df -h "$HOME" --output=avail | tail -1 | xargs)"

# ── Phase 1: Downloads (parallel) ────────────────────────────────────
if [ "$MODE" = "all" ]; then
    log "Phase 1: Downloads"

    bash "$SCRIPT_DIR/rasters/download-ghsl.sh" &> "$LOG_DIR/download-ghsl.log" &
    PID_GHSL=$!

    bash "$SCRIPT_DIR/rasters/download-copernicus-dem.sh" &> "$LOG_DIR/download-copernicus-dem.log" &
    PID_COP=$!

    log "  GHSL downloading (PID $PID_GHSL)"
    log "  Copernicus DEM downloading (PID $PID_COP)"

    # Wait for GHSL (fast, ~5 min)
    wait $PID_GHSL && log "  GHSL done" || log "  GHSL FAILED"

    # Don't wait for Copernicus DEM — it takes hours. Conversions can start now.
    log "  Copernicus DEM still downloading (PID $PID_COP) — continuing with conversions"
fi

# ── Phase 2: Convert existing data (parallel) ────────────────────────
log ""
log "Phase 2: Conversions (SRTM + WorldCover)"

bash "$SCRIPT_DIR/rasters/convert-dem-srtm.sh" &> "$LOG_DIR/convert-dem-srtm.log" &
PID_SRTM=$!

# IMD_FORCE=1 forwards --imd-force so an existing IMD tree gets rewritten —
# without it the converter skips existing tiles and a LUT change (e.g. the
# 2026-06 water=hard fix) silently never lands on already-converted hosts.
bash "$SCRIPT_DIR/rasters/convert-worldcover.sh" ${IMD_FORCE:+--imd-force} &> "$LOG_DIR/convert-worldcover.log" &
PID_WC=$!

log "  SRTM conversion (PID $PID_SRTM)"
log "  WorldCover → forest+imd (PID $PID_WC)"

FAIL=0
wait $PID_SRTM && log "  SRTM done" || { log "  SRTM FAILED"; FAIL=1; }
wait $PID_WC   && log "  WorldCover done" || { log "  WorldCover FAILED"; FAIL=1; }

# ── Phase 3: IMD overlay (after WorldCover) ──────────────────────────
log ""
log "Phase 3: IMD Copernicus overlay"
bash "$SCRIPT_DIR/rasters/convert-imd-overlay.sh" 2>&1 | sed 's/^/  /'

# ── Phase 4: Copernicus DEM conversion (if download finished) ────────
if [ "$MODE" = "all" ] && kill -0 $PID_COP 2>/dev/null; then
    log ""
    log "Phase 4: Copernicus DEM still downloading — conversion will run separately"
    log "  When done, run: bash scripts/rasters/convert-dem-copernicus.sh"
else
    log ""
    log "Phase 4: Converting Copernicus DEM"
    bash "$SCRIPT_DIR/rasters/convert-dem-copernicus.sh" &> "$LOG_DIR/convert-dem-copernicus.log"
    log "  Copernicus DEM conversion done"
fi

# ── Summary ──────────────────────────────────────────────────────────
T_TOTAL=$(( $(date +%s) - T_START ))
log ""
log "=== Summary ($(printf '%dh%02dm%02ds' $((T_TOTAL/3600)) $(((T_TOTAL%3600)/60)) $((T_TOTAL%60)))) ==="
log "  DEM (SRTM):      $(find data/prepared/dem/srtm -name '*.hgt' 2>/dev/null | wc -l) tiles"
log "  DEM (Copernicus): $(find data/prepared/dem/copernicus -name '*.hgt' 2>/dev/null | wc -l) tiles"
log "  Forest:           $(find data/prepared/rasters/forest -name '*.raw' 2>/dev/null | wc -l) tiles"
log "  IMD:              $(find data/prepared/rasters/imd -name '*.raw' 2>/dev/null | wc -l) tiles"
log "  Disk free:        $(df -h "$HOME" --output=avail | tail -1 | xargs)"

[ "$FAIL" -ne 0 ] && exit 1
exit 0
