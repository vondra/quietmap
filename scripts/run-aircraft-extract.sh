#!/usr/bin/env bash
# Build observed aircraft popup data under PREPARED_YEAR_DIR/z9/x/y.
# Required: PREPARED_YEAR_DIR (the year directory), PREPARED_DIR (root containing rasters/) and
# ADSB_CACHE, the primary provider archive (adsb.lol; its catalog.sqlite binds publisher assets).
# SECONDARY_ADSB_CACHE (ADSBexchange samples) is read on increment days only and adds what the
# primary provider did not receive. AIRCRAFT_ANCHOR=YYYY-MM selects the exposure year
# [anchor - 1 year, anchor): every day is a baseline candidate, its month-firsts are the increment
# candidates. Without an anchor, DAYS (and INCREMENT_DAYS) list the days explicitly. Days whose
# provider receipts fail are missing, never zero.
# --from-stage controls the first stage; successful upstream work stays available after failure.
# MEMMAX= explicitly disables the default 100G cgroup cap when the host does not offer user systemd.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR"

ADSB_CACHE="${ADSB_CACHE:-}"
SECONDARY_ADSB_CACHE="${SECONDARY_ADSB_CACHE:-}"
PREPARED_YEAR_DIR="${PREPARED_YEAR_DIR:-}"
PREPARED_DIR="${PREPARED_DIR:-}"
WORK_DIR="${WORK_DIR:-/tmp/aircraft-extract-work}"
DAYS="${DAYS:-}"
INCREMENT_DAYS="${INCREMENT_DAYS:-}"
SCOPE_BBOX="${SCOPE_BBOX:-}"
FROM_STAGE="${FROM_STAGE:-}"
UNTIL_STAGE="${UNTIL_STAGE:-}"
AIRCRAFT_ANCHOR="${AIRCRAFT_ANCHOR:-}"
MEMMAX="${MEMMAX-100G}"
MAX_THREADS="${MAX_THREADS:-}"

log() {
    local m
    m="[aircraft-extract] $(date '+%Y-%m-%d %H:%M:%S') $*"
    echo "$m"
    [ -n "${LOG_FILE:-}" ] && echo "$m" >>"$LOG_FILE"
    return 0
}
die() { log "ERROR: $*"; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --from-stage)
            [ $# -ge 2 ] || die "--from-stage requires a value"
            FROM_STAGE="$2"
            shift 2
            ;;
        --from-stage=*)
            FROM_STAGE="${1#*=}"
            shift
            ;;
        -h|--help)
            awk 'NR>1 && /^#/ {sub(/^# ?/,""); print; next} NR>1 {exit}' "$0"
            echo
            echo "Usage: $0 [--from-stage <stage0|...|stage2c>]"
            echo "Env vars: ADSB_CACHE, SECONDARY_ADSB_CACHE, PREPARED_YEAR_DIR (year directory containing z9/),"
            echo "          PREPARED_DIR, WORK_DIR, AIRCRAFT_ANCHOR or DAYS/INCREMENT_DAYS, SCOPE_BBOX,"
            echo "          FROM_STAGE, UNTIL_STAGE, LOG_DIR, MEMMAX, MAX_THREADS"
            exit 0
            ;;
        *)
            die "unknown argument: $1 (try --help)"
            ;;
    esac
done

[ -n "$PREPARED_YEAR_DIR" ] || die "requires PREPARED_YEAR_DIR= (year directory containing z9/)"
[ -n "$PREPARED_DIR" ] || die "requires PREPARED_DIR= (root containing rasters/dem, rasters/forest, rasters/imd)"
[ -n "$ADSB_CACHE" ] || die "requires ADSB_CACHE= with an explicit primary cache directory"

count_csv() { tr ',' '\n' <<<"$1" | wc -l; }

if [ -n "$AIRCRAFT_ANCHOR" ]; then
    [ -z "$DAYS$INCREMENT_DAYS" ] || die "the exposure year comes from one AIRCRAFT_ANCHOR, not DAYS/INCREMENT_DAYS"
    WINDOW_CSV="$(python3 "$SCRIPT_DIR/aircraft_window.py" --anchor "$AIRCRAFT_ANCHOR")" \
        || die "invalid aircraft sampling window"
    DAYS="${WINDOW_CSV%%$'\n'*}"
    INCREMENT_DAYS="${WINDOW_CSV#*$'\n'}"
    [ -n "$SECONDARY_ADSB_CACHE" ] || INCREMENT_DAYS=""
fi
[ -n "$DAYS" ] || die "requires AIRCRAFT_ANCHOR=YYYY-MM for the exposure year, or DAYS= for a subset"
[ -z "$INCREMENT_DAYS" ] || [ -n "$SECONDARY_ADSB_CACHE" ] \
    || die "INCREMENT_DAYS needs SECONDARY_ADSB_CACHE="

LOG_DIR="${LOG_DIR:-logs}"
LOG_FILE="$LOG_DIR/aircraft-extract-$(date '+%Y%m%d-%H%M%S').log"
mkdir -p "$LOG_DIR"
ln -sf "$(basename "$LOG_FILE")" "$LOG_DIR/aircraft-extract-latest.log"
log "logging to $LOG_FILE (symlinked $LOG_DIR/aircraft-extract-latest.log)"

log "rebuilding aircraft-extract (release)"
cargo build --release --manifest-path engine/aircraft-extract/Cargo.toml --bin aircraft-extract \
    2>&1 | stdbuf -oL -eL tee -a "$LOG_FILE"
BIN=./engine/target/release/aircraft-extract

GUARD=()
if [ -n "$MEMMAX" ]; then
    : "${XDG_RUNTIME_DIR:=/run/user/$(id -u)}"; export XDG_RUNTIME_DIR
    GUARD=(systemd-run --user --scope --quiet -p MemoryMax="$MEMMAX" -p MemorySwapMax=0)
    if ! command -v systemd-run >/dev/null 2>&1 || ! "${GUARD[@]}" true >/dev/null 2>&1; then
        die "MEMMAX=$MEMMAX set but the systemd-run --user MemoryMax guard is unavailable — refusing to run unguarded (an unbounded run global-OOM'd the whole session 2026-06-05). Re-run where user systemd is reachable, or set MEMMAX= to opt out."
    fi
    log "OOM guard: MemoryMax=$MEMMAX MemorySwapMax=0"
fi

SCOPE_ARGS=()
if [ -n "$SCOPE_BBOX" ]; then
    SCOPE_ARGS+=(--scope-bbox="$SCOPE_BBOX")
    log "scope bbox: $SCOPE_BBOX"
fi
THREAD_ARGS=()
if [ -n "$MAX_THREADS" ]; then
    THREAD_ARGS+=(--max-threads "$MAX_THREADS")
    log "max-threads: $MAX_THREADS (rayon pool cap — bounds concurrent mega-hub RAM)"
fi

mkdir -p "$WORK_DIR" "$PREPARED_YEAR_DIR"

stamp_gate() {
    log "publish gate: aircraft schema and sampling window"
    "$BIN" audit --prepared-year-dir "$PREPARED_YEAR_DIR" --segments-by-square "$1" \
        2>&1 | stdbuf -oL -eL tee -a "$LOG_FILE"
}

log "primary cache=$ADSB_CACHE secondary cache=${SECONDARY_ADSB_CACHE:-<none>} scope=${SCOPE_BBOX:-<global>}"
INCREMENT_COUNT=0
[ -z "$INCREMENT_DAYS" ] || INCREMENT_COUNT="$(count_csv "$INCREMENT_DAYS")"
log "$(count_csv "$DAYS") baseline candidate day(s); $INCREMENT_COUNT increment candidate day(s)"
EXTRA_ARGS=()
[ -z "$SECONDARY_ADSB_CACHE" ] || EXTRA_ARGS+=(--secondary-adsb-cache "$SECONDARY_ADSB_CACHE")
[ -z "$INCREMENT_DAYS" ] || EXTRA_ARGS+=(--increment-days "$INCREMENT_DAYS")
[ -z "$UNTIL_STAGE" ] || EXTRA_ARGS+=(--until-stage "$UNTIL_STAGE")
if [ -n "$FROM_STAGE" ]; then
    EXTRA_ARGS+=(--from-stage "$FROM_STAGE")
    log "from-stage: $FROM_STAGE (skipping every phase before $FROM_STAGE)"
fi
"${GUARD[@]}" "$BIN" run-all \
    --adsb-cache "$ADSB_CACHE" \
    --prepared-year-dir "$PREPARED_YEAR_DIR" \
    --prepared-dir "$PREPARED_DIR" \
    --work-dir "$WORK_DIR" \
    --days "$DAYS" \
    "${EXTRA_ARGS[@]}" \
    "${SCOPE_ARGS[@]}" \
    "${THREAD_ARGS[@]}" \
    2>&1 | stdbuf -oL -eL tee -a "$LOG_FILE"

stamp_gate "$WORK_DIR/segments_by_square"

log "done — popup arrows in $PREPARED_YEAR_DIR/z9/<x>/<y>/{airborne,cruise,airport_traffic}.arrow"
