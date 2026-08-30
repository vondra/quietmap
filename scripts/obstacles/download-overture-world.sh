#!/usr/bin/env bash
# Download the WORLD Overture buildings parquet cache for the vector obstacle
# ingest (geodata-v2 1.1 world extension).
#
#   scripts/obstacles/download-overture-world.sh [--jobs 6]
#
# Tile list comes from the VECTOR source itself (scripts/obstacles/world-tile-census.py
# over overture-obstacles/h3r4) — it used to be `ls prepared/rasters/building/*.raw`,
# which made the raster the source of truth for the vector ingest and blocked
# deleting it. Resumable: a tile whose parquet exists non-empty is skipped; overturemaps writes via a temp file so partials never
# count. ~80 GB total into data/enrichment/global/overture-buildings/parquet.
set -euo pipefail
cd "$(dirname "$0")/../.."

PARQUET_DIR="data/enrichment/global/overture-buildings/parquet"
JOBS=6
[ "${1:-}" = "--jobs" ] && JOBS="$2"

command -v overturemaps > /dev/null || { echo "overturemaps CLI missing" >&2; exit 1; }
mkdir -p "$PARQUET_DIR"

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
INGESTED_LIST="$(pwd)/data/enrichment/global/overture-obstacles/.ingested-tiles"
[ -f "$INGESTED_LIST" ] || INGESTED_LIST=""
export INGESTED_LIST

python3 scripts/obstacles/world-tile-census.py > /tmp/overture-world-tiles.txt
total=$(wc -l < /tmp/overture-world-tiles.txt)
done_n=$(ls "$PARQUET_DIR"/*.parquet 2>/dev/null | wc -l)
echo "[overture-world] $total tiles in census, $done_n cached → $PARQUET_DIR"

fail=0
xargs -P "$JOBS" -I{} bash -c 'fetch_one "$1"' _ {} < /tmp/overture-world-tiles.txt || fail=1
done_n=$(ls "$PARQUET_DIR"/*.parquet 2>/dev/null | wc -l)
echo "[overture-world] finished: $done_n/$total parquets (fail=$fail)"
exit "$fail"
