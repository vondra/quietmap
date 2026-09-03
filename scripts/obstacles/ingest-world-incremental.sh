#!/usr/bin/env bash
# Incremental world obstacle ingest (geodata-v2 1.1 world): watch the Overture
# parquet cache and run `ingest-overture-obstacles.py` on every tile exactly
# once, while the world download is still streaming in — per-tile ingest is
# independent by the centroid half-open ownership contract (see the ingest
# header), so staging pipelines behind the download instead of waiting ~a day
# for it to finish.
#
#   scripts/obstacles/ingest-world-incremental.sh [--jobs 6] [--tiles FILE]
#
# Resume-safe: ingested tiles are recorded in .ingested-tiles; a re-run skips
# them (the ingest's own stale-shard reconcile guards double-runs anyway).
# Exits when the downloader has finished AND every cached parquet is ingested.
set -euo pipefail
cd "$(dirname "$0")/../.."

SOURCE_ROOT="data/source/enrichment/global"
PARQUET_DIR="$SOURCE_ROOT/overture-buildings/parquet"
STATE="$SOURCE_ROOT/overture-obstacles/.ingested-tiles"
JOBS=6
TILE_LIST=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --jobs) JOBS="${2:?need job count}"; shift 2 ;;
        --tiles) TILE_LIST="${2:?need tile list}"; shift 2 ;;
        *) echo "usage: $0 [--jobs 1..16] [--tiles FILE]" >&2; exit 2 ;;
    esac
done
[[ "$JOBS" =~ ^([1-9]|1[0-6])$ ]] || { echo "jobs must be 1..16" >&2; exit 2; }

mkdir -p "$(dirname "$STATE")"
touch "$STATE"
SELECTED=$(mktemp)
trap 'rm -f "$SELECTED"' EXIT
if [ -n "$TILE_LIST" ]; then
    [ -r "$TILE_LIST" ] || { echo "tile list is unreadable: $TILE_LIST" >&2; exit 2; }
    if grep -Ev '^[NS][0-9]{2}[EW][0-9]{3}$' "$TILE_LIST" > /dev/null; then
        echo "tile list contains an invalid tile name: $TILE_LIST" >&2; exit 2
    fi
    sort -u "$TILE_LIST" > "$SELECTED"
fi

ingest_one() {
    local tile="$1"
    if nice -n 10 python3 scripts/obstacles/ingest-overture-obstacles.py "$tile" \
        >> "$SOURCE_ROOT/overture-obstacles/.ingest-runs.log" 2>&1; then
        echo "$tile" >> "$STATE"
        echo "[world-ingest] $tile ok"
    else
        echo "[world-ingest] $tile FAILED (left out of state; next pass retries)" >&2
        return 1
    fi
}
export -f ingest_one
export STATE

while true; do
    ls "$PARQUET_DIR"/*.parquet 2>/dev/null | sed 's/.*\///; s/\.parquet$//' | sort > /tmp/world-ingest-have.txt
    sort "$STATE" > /tmp/world-ingest-done.txt
    if [ -s "$SELECTED" ]; then
        comm -12 "$SELECTED" /tmp/world-ingest-have.txt > /tmp/world-ingest-candidates.txt
    else
        cp /tmp/world-ingest-have.txt /tmp/world-ingest-candidates.txt
    fi
    comm -23 /tmp/world-ingest-candidates.txt /tmp/world-ingest-done.txt > /tmp/world-ingest-todo.txt
    todo=$(wc -l < /tmp/world-ingest-todo.txt)
    if [ "$todo" -gt 0 ]; then
        echo "[world-ingest] $(date '+%H:%M') ingesting $todo new tiles ($(wc -l < /tmp/world-ingest-done.txt) done)"
        xargs -P "$JOBS" -I{} bash -c 'ingest_one "$1"' _ {} < /tmp/world-ingest-todo.txt || true
    fi
    if ! pgrep -f "download-overture-world" > /dev/null && [ "$todo" -eq 0 ]; then
        if [ -s "$SELECTED" ]; then
            sort -u /tmp/world-ingest-have.txt /tmp/world-ingest-done.txt > /tmp/world-ingest-seen.txt
            comm -23 "$SELECTED" /tmp/world-ingest-seen.txt > /tmp/world-ingest-missing.txt
            if [ -s /tmp/world-ingest-missing.txt ]; then
                echo "[world-ingest] selected download missing: $(wc -l < /tmp/world-ingest-missing.txt)" >&2
                exit 1
            fi
        fi
        echo "[world-ingest] downloader gone and nothing left — finished: $(wc -l < "$STATE") tiles ingested"
        break
    fi
    sleep 300
done
