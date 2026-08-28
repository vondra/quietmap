#!/usr/bin/env bash
# Aircraft pipeline driver — wrapper around the `aircraft-extract`
# binary that runs Stage 0..2C end-to-end against the ADS-B TAR cache
# and writes per-R4 popup arrows (airborne / cruise / ground) under
# `data/prepared/{DATA_YEAR}/h3r4/<R4>/`.
#
# Stage 0/1 are per-day; Stage 2A/2B/2C aggregate the full window.
# `aircraft-extract run-all` orchestrates the cross-day flow. The
# orchestrator REFUSES to start when `--work-dir` already holds
# outputs (flights/, segments/, segments_by_r4/) AND `--from-stage`
# is the default `stage0` — without that guard, re-running this
# wrapper would silently overwrite hours of cached upstream work that
# the operator almost certainly meant to reuse. Pick `--from-stage
# stageX` (or set `FROM_STAGE=stageX`) for the stage whose code you
# changed, OR `rm -rf $WORK_DIR` to start fresh.
#
# Stage reuse — to iterate on a single later stage without re-running
# upstream work, pass `--from-stage <stage>` (or set
# `FROM_STAGE=<stage>`). Valid values: stage0 (default — full
# pipeline), stage1, shuffle, stage1-5, stage2a, stage2b, stage2c.
# Each variant reuses outputs that an earlier orchestrator run left
# under WORK_DIR (`flights/`, `segments/`, `segments_by_r4/`). Example:
# `./scripts/run-aircraft-extract.sh --from-stage stage2a` reuses the
# cached Stage 1 segments and per-R4 shuffle, runs only Stage 2A/2B/2C.
#
# HYBRID two-window extract (HYBRID=1).
# GA + helicopter classes use every available adsb.lol day while airline
# traffic keeps the 12-day first-of-month window, so a one-off GA flight is
# divided by its GA-window day count instead of the airline-window day count
# (kills the ×30 / +14.8 dB phantom from the Kytín audit 2026-06-12). Three invocations:
#   pass J (airline): --feed $AIRLINE_FEED --class-filter non-ga --until-stage stage1 → $WORK_DIR/airline
#   pass G (GA):      --feed adsblol      --class-filter ga     --until-stage stage1 → $WORK_DIR/ga
#   merge:            --work-dir $WORK_DIR/airline --ga-segments-dir $WORK_DIR/ga/segments --from-stage shuffle
# A pass is SKIPPED when its `segments/` is already populated (resume
# semantics — `rm -rf` that pass's work dir to redo it; a pass that
# crashed mid-Stage-0 also needs the rm, the binary's populated-dir
# guard explains the options). FROM_STAGE, when set, applies to the
# MERGE invocation only (default shuffle; stage2a/2b/2c iterate on
# Stage 2 against the existing hybrid shuffle). DAYS is rejected in
# hybrid mode — set AIRLINE_DAYS / GA_DAYS instead (both default to
# every day present in their cache). Hybrid env knobs: AIRLINE_FEED
# (default adsbexchange), AIRLINE_CACHE (the airline feed cache on the ops
# storage host), AIRLINE_DAYS, GA_CACHE (the raw adsb.lol archive on
# the ADS-B archive host), GA_DAYS, FAIL_ON_GA_CRUISE=1
# (merge hard-fails if GA classes leak into cruise).
# Cross-host runbook (plan §4.2): run pass G on the ADS-B archive host
# (data-local), rsync $WORK_DIR/ga/segments to the compute host, run
# pass J + merge there.
#
# Single-feed defaults: --feed adsblol reads the raw adsb.lol archive
# on the ADS-B archive host (release naming v{Y.M.D}-planes-readsb-prod-0
# is resolved by the binary). The old Praha dev subset stays reachable
# via ADSB_CACHE=data/source/flights-cache/radius/praha-150km plus
# SCOPE_BBOX=48.65,12.00,51.55,16.90 (subset caches REQUIRE a scope).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR"

DATA_YEAR="${DATA_YEAR:-$(python3 -c 'import json;print(json.load(open("scripts/dataset-year.json"))["current_year"])')}"  # default from committed ./DATA_YEAR (bump there yearly); env overrides
DATA_ROOT="${DATA_ROOT:-data}"
# Which ADS-B network to read. adsb.lol and adsbexchange ship the identical
# readsb trace_full TAR format, so --feed only picks the default cache path +
# scope and stamps provenance. ADSB_CACHE / SCOPE_BBOX (if set) win over the
# per-feed defaults resolved after arg parsing.
FEED="${FEED:-adsblol}"
ADSB_CACHE="${ADSB_CACHE-__PER_FEED__}"
H3R4_DIR="${H3R4_DIR:-$DATA_ROOT/prepared/$DATA_YEAR/h3r4}"
PREPARED_DIR="${PREPARED_DIR:-$DATA_ROOT/prepared}"
WORK_DIR="${WORK_DIR:-/tmp/aircraft-extract-work}"
DAYS="${DAYS:-}"
# REQUIRED for bbox/radius subset caches (Canary, Praha-150km, ...) —
# full daily traces of in-scope flights would otherwise overwrite
# global R4 files. The aircraft-extract binary hard-fails when
# --adsb-cache contains /bbox/ or /radius/ AND --scope-bbox is unset.
#
# Resolved per --feed below (both feeds default global). An explicit
# SCOPE_BBOX env (even empty) wins.
SCOPE_BBOX="${SCOPE_BBOX-__PER_FEED__}"
FROM_STAGE="${FROM_STAGE:-}"
# Hybrid two-window extract — see the HYBRID header section.
HYBRID="${HYBRID:-}"
AIRLINE_FEED="${AIRLINE_FEED:-adsbexchange}"
AIRLINE_CACHE="${AIRLINE_CACHE:-/storagebox/adsbexchange}"
AIRLINE_DAYS="${AIRLINE_DAYS:-}"
GA_CACHE="${GA_CACHE:-/mnt/data/adsb}"
GA_DAYS="${GA_DAYS:-}"
FAIL_ON_GA_CRUISE="${FAIL_ON_GA_CRUISE:-}"
# OOM guard. Stage 2B/2C load multi-million-segment mega-hub R4s; an
# unbounded run peaked at 126 GB and triggered a GLOBAL oom-kill that
# took down the whole session (2026-06-05). systemd-run --scope confines
# the blast radius to MemoryMax so only THIS job dies, never the box.
# MAX_THREADS caps rayon so N concurrent mega-hub cells can't co-resident
# past the cap. MEMMAX= (empty) opts out (e.g. no user systemd).
MEMMAX="${MEMMAX:-100G}"
MAX_THREADS="${MAX_THREADS:-}"

# Echo to stdout AND (once LOG_FILE is set) append to it, so status the
# binary doesn't emit — the OOM-guard decision above all — survives a
# `nohup …>/dev/null` launch where the script's own stdout is discarded.
log() {
    local m="[aircraft-extract] $(date '+%Y-%m-%d %H:%M:%S') $*"
    echo "$m"
    [ -n "${LOG_FILE:-}" ] && echo "$m" >>"$LOG_FILE"
    return 0
}
die() { log "ERROR: $*"; exit 1; }

# CLI args are accepted as the discoverable alternative to env vars.
# Only the stage-reuse flag is parsed here — every other knob remains
# env-var-driven to keep the surface small (DAYS, ADSB_CACHE, … rarely
# change per invocation, --from-stage flips between runs).
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
            # Pipe the header comment block (between the shebang and
            # the first non-comment line) so `--help` and the source
            # docs stay one source of truth.
            awk 'NR>1 && /^#/ {sub(/^# ?/,""); print; next} NR>1 {exit}' "$0"
            echo
            echo "Usage: $0 [--feed <adsblol|adsbexchange>] [--from-stage <stage0|...|stage2c>]"
            echo "Env vars: DATA_YEAR, DATA_ROOT, FEED, ADSB_CACHE, H3R4_DIR, PREPARED_DIR, WORK_DIR,"
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

# Per-feed defaults — applied only where ADSB_CACHE / SCOPE_BBOX weren't set.
case "$FEED" in
    adsblol)      FEED_CACHE="/mnt/data/adsb";          FEED_SCOPE="" ;;
    adsbexchange) FEED_CACHE="/storagebox/adsbexchange"; FEED_SCOPE="" ;;
    *)            die "unknown --feed: $FEED (adsblol|adsbexchange)" ;;
esac
[ "$ADSB_CACHE" = "__PER_FEED__" ] && ADSB_CACHE="$FEED_CACHE"
[ "$SCOPE_BBOX" = "__PER_FEED__" ] && SCOPE_BBOX="$FEED_SCOPE"

# Day list from a cache dir: every day holding a .tar (or .tar.aa —
# summer adsb.lol days ship ONLY split parts, a bare-.tar find skips
# them). Handles both plain `<year>/<day>/` layouts and the raw
# adsb.lol release naming `<year>/v{Y.M.D}-planes-readsb-prod-0/`
# (incl. the upstream `…prod-0tmp` tag variant on 15 days of 2025-05/06
# — complete downloads, verified), mapped back to Y-M-D for --days.
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
./scripts/ensure-engine-target-shims.sh
cargo build --release --manifest-path engine/aircraft-extract/Cargo.toml --bin aircraft-extract \
    2>&1 | stdbuf -oL -eL tee -a "$LOG_FILE"
BIN=./engine/target/release/aircraft-extract

# Confine RAM to MEMMAX via a transient user scope so an OOM kills only
# this job, not the box. --scope runs synchronously (foreground), so the
# `tee` pipe + live progress are unchanged. Probe first; if user systemd
# is unreachable we REFUSE to run rather than silently fall back to the
# unguarded launch that already global-OOM'd the box — set MEMMAX= to
# explicitly opt out of the guard.
GUARD=()
if [ -n "$MEMMAX" ]; then
    : "${XDG_RUNTIME_DIR:=/run/user/$(id -u)}"; export XDG_RUNTIME_DIR
    GUARD=(systemd-run --user --scope --quiet -p MemoryMax="$MEMMAX" -p MemorySwapMax=0)
    # Probe with the REAL guard (incl. the -p caps) so a host that can't
    # honour MemoryMax/MemorySwapMax fails HERE, not mid-pipe.
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

mkdir -p "$WORK_DIR" "$H3R4_DIR"

# Publish gate (2026-07-28): a partial run must never leave the tree with
# mixed `sample_days_by_class` stamps — the reader's only consistency check
# fired as a user-visible popup 500 for 9 days (157 stale cells found by the
# owner, 2026-07-18 run). Sweep the whole tree; a mixed result fails the run
# HERE, in ops, not in someone's popup.
stamp_gate() {
    log "publish gate: aircraft stamp consistency sweep (pipeline/audit-aircraft-stamps.ts)"
    # Local tsx from pipeline/, NOT npx (races + wrong resolution from repo root).
    # The audit resolves a repo-relative H3R4_DIR against the repo root itself.
    if ! (cd "$PROJECT_DIR/pipeline" && H3R4_DIR="$H3R4_DIR" DATA_YEAR="$DATA_YEAR" node_modules/.bin/tsx audit-aircraft-stamps.ts --quiet) 2>&1 | stdbuf -oL -eL tee -a "$LOG_FILE"; then
        die "stamp gate FAILED — aircraft arrows not publishable (see $LOG_FILE)"
    fi
}

if [ -n "$HYBRID" ]; then
    # Both hybrid passes stop after Stage 1; one merge invocation shuffles
    # airline ∪ GA
    # day shards and runs Stage 1.5/2A/2B/2C. `tee` streams every
    # milestone into the shared log, same as the single-feed flow.
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
        if [ -n "$(ls -A "$wd/segments" 2>/dev/null)" ]; then
            # A crashed pass leaves a PARTIAL segments/ that a bare populated-dir
            # check would silently accept — the merge would then ship a wrong
            # sample basis (`ga_n_days` counted from the partial set). The
            # day-file count must match the request exactly.
            local have want
            have=$(find "$wd/segments" -maxdepth 1 -name '*.arrow' | wc -l)
            want=$(count_csv "$days")
            [ "$have" -eq "$want" ] \
                || die "pass $label: $wd/segments holds $have day file(s) but $want requested — partial pass; rm -rf $wd to redo"
            log "pass $label: $wd/segments complete ($have/$want days) — skipping (rm -rf $wd to redo)"
            return 0
        fi
        log "pass $label: feed=$feed cache=$cache class-filter=$filter until-stage=stage1 → $wd"
        "${GUARD[@]}" "$BIN" run-all \
            --adsb-cache "$cache" \
            --h3r4-dir "$H3R4_DIR" \
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
        --h3r4-dir "$H3R4_DIR" \
        --prepared-dir "$PREPARED_DIR" \
        --work-dir "$W_AIR" \
        --days "$AIRLINE_DAYS" \
        --feed "$AIRLINE_FEED" \
        "${MERGE_ARGS[@]}" \
        "${SCOPE_ARGS[@]}" \
        "${THREAD_ARGS[@]}" \
        2>&1 | stdbuf -oL -eL tee -a "$LOG_FILE"

    stamp_gate

    log "done — hybrid popup arrows in $H3R4_DIR/<R4>/{airborne,cruise,airport_traffic}.arrow"
    exit 0
fi

log "feed: $FEED  cache=$ADSB_CACHE  scope=${SCOPE_BBOX:-<global>}"
if [ -z "$DAYS" ]; then
    log "DAYS env var not set; deriving from ADSB_CACHE=$ADSB_CACHE"
    DAYS="$(derive_days "$ADSB_CACHE")"
fi
[ -n "$DAYS" ] || die "no ADS-B TAR days resolved from $ADSB_CACHE"
# The adsblol default cache is now the full global archive — a bare
# `./run-aircraft-extract.sh` would silently start a multi-hour TB-scan where
# the old default was the 14-day Praha subset. Deriving a big window without
# explicit DAYS needs an acknowledgement.
if [ "$(count_csv "$DAYS")" -gt 60 ] && [ "${ALLOW_FULL_ARCHIVE:-}" != 1 ]; then
    die "derived $(count_csv "$DAYS") day(s) from $ADSB_CACHE — full-archive run. Set DAYS=… for a subset, or ALLOW_FULL_ARCHIVE=1 to confirm."
fi

log "running aircraft-extract run-all (DAYS=$DAYS)"
# `tee` instead of a `tail` filter: streams every per-day milestone +
# Stage 2B/2C 10-second progress tick straight into the log file AND
# stdout (so `bash run_in_background` output and a foreground terminal
# both see live progress). `tail -F logs/aircraft-extract-latest.log`
# is the operator's go-to during multi-hour global runs.
EXTRA_ARGS=(--feed "$FEED")
if [ -n "$FROM_STAGE" ]; then
    EXTRA_ARGS+=(--from-stage "$FROM_STAGE")
    log "from-stage: $FROM_STAGE (skipping every phase before $FROM_STAGE)"
fi
"${GUARD[@]}" "$BIN" run-all \
    --adsb-cache "$ADSB_CACHE" \
    --h3r4-dir "$H3R4_DIR" \
    --prepared-dir "$PREPARED_DIR" \
    --work-dir "$WORK_DIR" \
    --days "$DAYS" \
    "${EXTRA_ARGS[@]}" \
    "${SCOPE_ARGS[@]}" \
    "${THREAD_ARGS[@]}" \
    2>&1 | stdbuf -oL -eL tee -a "$LOG_FILE"

stamp_gate

log "done — popup arrows in $H3R4_DIR/<R4>/{airborne,cruise,airport_traffic}.arrow"
