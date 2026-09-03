#!/usr/bin/env bash
# Download the WORLD Overture buildings parquet cache for the vector obstacle
# ingest (geodata-v2 1.1 world extension).
#
#   scripts/obstacles/download-overture-world.sh [--jobs 6] [--tiles FILE]
#
# The default comes from the Planet-extracted R4 inventory, not the obstacle
# tree: a shard-less land cell is precisely the empty case this job must ingest.
# --tiles is the additive recovery path for a measured gap.
set -euo pipefail
cd "$(dirname "$0")/../.."

SOURCE_ROOT="data/source/enrichment/global"
PARQUET_DIR="$SOURCE_ROOT/overture-buildings/parquet"
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

command -v overturemaps > /dev/null || { echo "overturemaps CLI missing" >&2; exit 1; }
mkdir -p "$PARQUET_DIR"
TILES=$(mktemp)
trap 'rm -f "$TILES"' EXIT
if [ -n "$TILE_LIST" ]; then
    [ -r "$TILE_LIST" ] || { echo "tile list is unreadable: $TILE_LIST" >&2; exit 2; }
    if grep -Ev '^[NS][0-9]{2}[EW][0-9]{3}$' "$TILE_LIST" > /dev/null; then
        echo "tile list contains an invalid tile name: $TILE_LIST" >&2; exit 2
    fi
    sort -u "$TILE_LIST" > "$TILES"
else
    python3 scripts/obstacles/world-tile-census.py > "$TILES"
fi
[ -s "$TILES" ] || { echo "tile list is empty" >&2; exit 2; }

fetch_one() {
    local tile="$1"
    local out="$PARQUET_DIR/$tile.parquet"
    [ -s "$out" ] && return 0
    # An INGESTED tile's parquet is deliberately deleted (space hygiene) —
    # never re-download spent tiles.
    [ -n "${INGESTED_LIST:-}" ] && grep -qx "$tile" "$INGESTED_LIST" 2>/dev/null && return 0
    # N50E014 → bbox 14,50,15,51 (lon_min,lat_min,lon_max,lat_max)
    local ns="${tile:0:1}" lat="${tile:1:2}" ew="${tile:3:1}" lon="${tile:4:3}"
    lat=$((10#$lat)); lon=$((10#$lon))
    [ "$ns" = "S" ] && lat=$((-lat))
    [ "$ew" = "W" ] && lon=$((-lon))
    local tmp="$out.dl"
    if timeout 1800 overturemaps download \
        --bbox "$lon,$lat,$((lon + 1)),$((lat + 1))" \
        -f geoparquet --type building -o "$tmp" 2>> "$PARQUET_DIR/.errors.log"; then
        mv "$tmp" "$out"
        echo "[overture-world] $tile ($(stat -c%s "$out" | numfmt --to=iec))"
    else
        rm -f "$tmp"
        echo "[overture-world] $tile FAILED (will retry on next run)" >&2
        return 1
    fi
}
export -f fetch_one
export PARQUET_DIR
INGESTED_LIST="$(pwd)/$SOURCE_ROOT/overture-obstacles/.ingested-tiles"
[ -f "$INGESTED_LIST" ] || INGESTED_LIST=""
export INGESTED_LIST

total=$(wc -l < "$TILES")
done_n=$(ls "$PARQUET_DIR"/*.parquet 2>/dev/null | wc -l)
echo "[overture-world] $total selected tiles, $done_n cached → $PARQUET_DIR"

fail=0
xargs -P "$JOBS" -I{} bash -c 'fetch_one "$1"' _ {} < "$TILES" || fail=1
done_n=$(ls "$PARQUET_DIR"/*.parquet 2>/dev/null | wc -l)
echo "[overture-world] finished: $done_n/$total parquets (fail=$fail)"
exit "$fail"
