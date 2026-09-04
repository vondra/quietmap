#!/usr/bin/env bash
# Build observed aircraft popup data under PREPARED_YEAR_DIR/z9/x/y.
# Required: PREPARED_YEAR_DIR (the year directory), PREPARED_DIR (root containing rasters/),
# ADSB_CACHE and optional DAYS (comma-separated YYYY-MM-DD; defaults to cache days).
# HYBRID=1 uses AIRLINE_CACHE/AIRLINE_DAYS for non-GA (including GSE), and
# GA_CACHE/GA_DAYS for piston GA and helicopters. Each pass preserves its own
# sampling denominator; the merge follows the proven dev1 class routing.
# Existing passes are reused only after exact day, schema, class and feed checks.
# --from-stage controls the single pass or hybrid merge stage; successful
# upstream work stays available after failure. MEMMAX= explicitly disables
# the default 100G cgroup cap when the host does not offer user systemd.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR"

FEED="${FEED:-adsblol}"
ADSB_CACHE="${ADSB_CACHE:-}"
PREPARED_YEAR_DIR="${PREPARED_YEAR_DIR:-}"
PREPARED_DIR="${PREPARED_DIR:-}"
WORK_DIR="${WORK_DIR:-/tmp/aircraft-extract-work}"
DAYS="${DAYS:-}"
SCOPE_BBOX="${SCOPE_BBOX:-}"
FROM_STAGE="${FROM_STAGE:-}"
HYBRID="${HYBRID:-}"
AIRLINE_FEED="${AIRLINE_FEED:-adsbexchange}"
AIRLINE_CACHE="${AIRLINE_CACHE:-}"
AIRLINE_DAYS="${AIRLINE_DAYS:-}"
GA_CACHE="${GA_CACHE:-}"
GA_DAYS="${GA_DAYS:-}"
FAIL_ON_GA_CRUISE="${FAIL_ON_GA_CRUISE:-}"
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
        --feed)
            [ $# -ge 2 ] || die "--feed requires a value (adsblol|adsbexchange)"
            FEED="$2"
            shift 2
            ;;
        --feed=*)
            FEED="${1#*=}"
            shift
            ;;
        -h|--help)
            awk 'NR>1 && /^#/ {sub(/^# ?/,""); print; next} NR>1 {exit}' "$0"
            echo
            echo "Usage: $0 [--feed <adsblol|adsbexchange>] [--from-stage <stage0|...|stage2c>]"
            echo "Env vars: FEED, ADSB_CACHE, PREPARED_YEAR_DIR (year directory containing z9/), PREPARED_DIR, WORK_DIR,"
            echo "          DAYS, SCOPE_BBOX, FROM_STAGE, LOG_DIR, MEMMAX, MAX_THREADS"
            echo "Hybrid:   HYBRID=1, AIRLINE_FEED, AIRLINE_CACHE, AIRLINE_DAYS, GA_CACHE, GA_DAYS,"
            echo "          FAIL_ON_GA_CRUISE=1"
            exit 0
            ;;
        *)
            die "unknown argument: $1 (try --help)"
            ;;
    esac
done

[ -n "$PREPARED_YEAR_DIR" ] || die "requires PREPARED_YEAR_DIR= (year directory containing z9/)"
[ -n "$PREPARED_DIR" ] || die "requires PREPARED_DIR= (root containing rasters/dem, rasters/forest, rasters/imd)"
case "$FEED" in
    adsblol|adsbexchange) ;;
    *)            die "unknown --feed: $FEED (adsblol|adsbexchange)" ;;
esac
if [ -n "$HYBRID" ]; then
    [ -n "$AIRLINE_CACHE" ] || die "HYBRID=1 requires AIRLINE_CACHE= with an explicit cache directory"
    [ -n "$GA_CACHE" ] || die "HYBRID=1 requires GA_CACHE= with an explicit cache directory"
else
    [ -n "$ADSB_CACHE" ] || die "requires ADSB_CACHE= with an explicit cache directory"
fi

derive_days() {
    local cache="$1"
    [ -d "$cache" ] || die "$cache not found and no day list provided"
    find "$cache" -mindepth 1 -maxdepth 4 \( -name '*.tar' -o -name '*.tar.aa' \) -printf '%h\n' \
        | awk -F/ '{print $NF}' \
        | sed -E 's/^v([0-9]{4})\.([0-9]{2})\.([0-9]{2})-planes-readsb-prod-0(tmp)?$/\1-\2-\3/' \
        | sort -u | paste -sd,
}
count_csv() { tr ',' '\n' <<<"$1" | wc -l; }

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
    log "publish gate: aircraft schema and both sampling windows"
    "$BIN" audit --prepared-year-dir "$PREPARED_YEAR_DIR" --segments-by-square "$1" \
        2>&1 | stdbuf -oL -eL tee -a "$LOG_FILE"
}

if [ -n "$HYBRID" ]; then
    [ -z "$DAYS" ] || die "HYBRID=1 ignores DAYS — set AIRLINE_DAYS / GA_DAYS instead"
    W_AIR="$WORK_DIR/airline"
    W_GA="$WORK_DIR/ga"
    if [ -z "$AIRLINE_DAYS" ]; then
        log "AIRLINE_DAYS not set; deriving from AIRLINE_CACHE=$AIRLINE_CACHE"
        AIRLINE_DAYS="$(derive_days "$AIRLINE_CACHE")"
    fi
    if [ -z "$GA_DAYS" ]; then
        log "GA_DAYS not set; deriving from GA_CACHE=$GA_CACHE"
        GA_DAYS="$(derive_days "$GA_CACHE")"
    fi
    [ -n "$AIRLINE_DAYS" ] || die "no airline ADS-B TAR days resolved from $AIRLINE_CACHE"
    [ -n "$GA_DAYS" ] || die "no GA ADS-B TAR days resolved from $GA_CACHE"
    log "hybrid: airline $(count_csv "$AIRLINE_DAYS") day(s) from $AIRLINE_CACHE (feed=$AIRLINE_FEED) + GA $(count_csv "$GA_DAYS") day(s) from $GA_CACHE (feed=adsblol)"

    run_pass() { # <label> <feed> <cache> <days> <class-filter> <work-dir>
        local label="$1" feed="$2" cache="$3" days="$4" filter="$5" wd="$6"
        if [ -d "$wd/segments" ] && [ -n "$(find "$wd/segments" -mindepth 1 -maxdepth 1 -print -quit)" ]; then
            "$BIN" validate-segments --segments-dir "$wd/segments" --days "$days" \
                --class-filter "$filter" --feed "$feed" \
                2>&1 | stdbuf -oL -eL tee -a "$LOG_FILE"
            log "pass $label: complete typed day set verified — reusing $wd/segments"
            return 0
        fi
        log "pass $label: feed=$feed cache=$cache class-filter=$filter until-stage=stage1 → $wd"
        "${GUARD[@]}" "$BIN" run-all \
            --adsb-cache "$cache" \
            --prepared-year-dir "$PREPARED_YEAR_DIR" \
            --prepared-dir "$PREPARED_DIR" \
            --work-dir "$wd" \
            --days "$days" \
            --feed "$feed" \
            --class-filter "$filter" \
            --until-stage stage1 \
            "${SCOPE_ARGS[@]}" \
            "${THREAD_ARGS[@]}" \
            2>&1 | stdbuf -oL -eL tee -a "$LOG_FILE"
    }

    run_pass J "$AIRLINE_FEED" "$AIRLINE_CACHE" "$AIRLINE_DAYS" non-ga "$W_AIR"
    run_pass G adsblol "$GA_CACHE" "$GA_DAYS" ga "$W_GA"

    MERGE_ARGS=(--from-stage "${FROM_STAGE:-shuffle}" --ga-segments-dir "$W_GA/segments")
    [ -n "$FAIL_ON_GA_CRUISE" ] && MERGE_ARGS+=(--fail-on-ga-cruise)
    log "merge: airline work-dir $W_AIR + GA segments $W_GA/segments (from-stage ${FROM_STAGE:-shuffle})"
    "${GUARD[@]}" "$BIN" run-all \
        --adsb-cache "$AIRLINE_CACHE" \
        --prepared-year-dir "$PREPARED_YEAR_DIR" \
        --prepared-dir "$PREPARED_DIR" \
        --work-dir "$W_AIR" \
        --days "$AIRLINE_DAYS" \
        --feed "$AIRLINE_FEED" \
        "${MERGE_ARGS[@]}" \
        "${SCOPE_ARGS[@]}" \
        "${THREAD_ARGS[@]}" \
        2>&1 | stdbuf -oL -eL tee -a "$LOG_FILE"

    stamp_gate "$W_AIR/segments_by_square"

    log "done — hybrid popup arrows in $PREPARED_YEAR_DIR/z9/<x>/<y>/{airborne,cruise,airport_traffic}.arrow"
    exit 0
fi

log "feed: $FEED  cache=$ADSB_CACHE  scope=${SCOPE_BBOX:-<global>}"
if [ -z "$DAYS" ]; then
    log "DAYS env var not set; deriving from ADSB_CACHE=$ADSB_CACHE"
    DAYS="$(derive_days "$ADSB_CACHE")"
fi
[ -n "$DAYS" ] || die "no ADS-B TAR days resolved from $ADSB_CACHE"
if [ "$(count_csv "$DAYS")" -gt 60 ] && [ "${ALLOW_FULL_ARCHIVE:-}" != 1 ]; then
    die "derived $(count_csv "$DAYS") day(s) from $ADSB_CACHE — full-archive run. Set DAYS=… for a subset, or ALLOW_FULL_ARCHIVE=1 to confirm."
fi

log "running aircraft-extract run-all (DAYS=$DAYS)"
EXTRA_ARGS=(--feed "$FEED")
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
