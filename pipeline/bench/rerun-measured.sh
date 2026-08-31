#!/usr/bin/env bash
# Re-run all measured enrichment scripts (roads + railways + buildings + industrial).
# Parallel with limited concurrency to avoid thrashing Arrow flock.
#
# Usage: DATA_YEAR=2026 bash pipeline/bench/rerun-measured.sh
set -uo pipefail   # NO -e: we want to continue even if one script fails
cd "$(dirname "$0")/../.."

LOG_DIR="logs/rerun-measured"
mkdir -p "$LOG_DIR"

YEAR="${DATA_YEAR:-2026}"
export DATA_YEAR="$YEAR"
JOBS="${JOBS:-6}"

# Bump Node heap to 8 GB. Some national enrichers hold the full country's
# cache in memory (e.g. enrich-buildings-es loads ~30M Catastro buildings)
# and would otherwise OOM at the default 3.4 GB limit.
export NODE_OPTIONS="${NODE_OPTIONS:---max-old-space-size=8192}"

log() { echo "[$(date '+%H:%M:%S')] $*"; }

# Helper: run one script, redirect logs
run_one() {
    local script="$1"
    local basename
    basename=$(basename "$script" .ts)
    local log="$LOG_DIR/${basename}.log"
    if ! npx tsx "$script" --enrich-only > "$log" 2>&1; then
        # Retry once without --enrich-only (maybe cache missing)
        if ! npx tsx "$script" >> "$log" 2>&1; then
            echo "FAIL $script" >> "$LOG_DIR/_failures.txt"
        fi
    fi
    echo "  done $basename"
}
export -f run_one
export DATA_YEAR LOG_DIR

# ── Discover surviving scripts ──
# service-tree is excluded: it is an estimated (not measured) source and is
# already run at the tail of scripts/osm-to-h3r4.sh immediately after the
# planet extract, where it has the freshest road_class / oneway state. Re-
# running it here would duplicate a 3 h job. rerun-measured handles ONLY
# measured / census / cadastre enrichers.
# continuity-fill is also excluded here and run in Phase 4 instead: it is a
# post-measured pass (needs every national + city anchor in place first), not a
# measured source, so it must run AFTER Phase 2/3, never alongside them.
ROADS=$(ls pipeline/enrich-roads-*.ts 2>/dev/null | grep -vE '/enrich-roads-(service-tree|continuity-fill)\.ts$')
RAILWAYS=$(ls pipeline/enrich-railway-*.ts 2>/dev/null)
BUILDINGS=$(ls pipeline/enrich-buildings-*.ts 2>/dev/null)
INDUSTRIAL=$(ls pipeline/enrich-industrial-*.ts 2>/dev/null)
GLOBAL=$(ls pipeline/enrich-global-industrial.ts pipeline/enrich-global-windturbines.ts 2>/dev/null)

log "Scripts to run:"
log "  roads:      $(echo "$ROADS" | wc -l)"
log "  railways:   $(echo "$RAILWAYS" | wc -l)"
log "  buildings:  $(echo "$BUILDINGS" | wc -l)"
log "  industrial: $(echo "$INDUSTRIAL" | wc -l)"
log "  global:     $(echo "$GLOBAL" | wc -l)"
log "  parallelism: $JOBS"

rm -f "$LOG_DIR/_failures.txt"

# ── Phase 1: Continental + Global (dependencies for country priority) ──
log ""
log "Phase 1: Global + Continental"
for s in $GLOBAL pipeline/enrich-roads-europe.ts pipeline/enrich-railway-europe.ts; do
    [ -f "$s" ] && run_one "$s"
done

# ── Phase 2: Country-level (parallel, all layers mixed) ──
log ""
log "Phase 2: Country scripts (parallel, $JOBS jobs)"
echo "$ROADS $RAILWAYS $BUILDINGS $INDUSTRIAL" | tr ' ' '\n' | grep -v '^$' | \
    xargs -P "$JOBS" -I{} bash -c 'run_one "$@"' _ {}

# ── Phase 3: City enrichers (SEQUENTIAL — outside the Phase-2 glob and
# after it on purpose: the city driver writes the same hex arrows national
# enrichers touch; racing them in the xargs pool would fight the per-hex
# lock. Rank ordering makes the result identical either way; running last
# just avoids wasted writes.) ──
log ""
log "Phase 3: City enrichers (sequential)"
run_one pipeline/enrich-cities-roads.ts

# ── Phase 4: Continuity fill (junction-bounded same-ref) — AFTER all measured
# road enrichers (national + city) so every anchor is in place. Copies a measured
# AADT across same-ref no-junction gaps on major roads (0-4); independent of
# service-tree (5-9). Per-hex self-contained → sharded like service-tree
# (osm-to-h3r4.sh) so the full planet finishes in minutes not hours. ──
log ""
log "Phase 4: Continuity fill (sharded x $JOBS)"
CONT_SHARDS="${CONT_SHARDS:-96}"
seq 0 $((CONT_SHARDS - 1)) | xargs -P "$JOBS" -I{} bash -c \
    'SHARD="$1/'"$CONT_SHARDS"'" DATA_YEAR="'"$YEAR"'" npx tsx pipeline/enrich-roads-continuity-fill.ts > "'"$LOG_DIR"'/continuity-fill-$1.log" 2>&1' _ {}
cont_filled=$(grep -h "segments filled" "$LOG_DIR"/continuity-fill-*.log 2>/dev/null | grep -oE "^  [0-9]+" | awk '{s+=$1} END {print s+0}')
log "  continuity-fill: $cont_filled segments filled"

# ── Summary ──
log ""
log "Done. Failures:"
if [ -f "$LOG_DIR/_failures.txt" ]; then
    cat "$LOG_DIR/_failures.txt"
    log "  ($(wc -l < "$LOG_DIR/_failures.txt") failures)"
else
    log "  (none)"
fi
