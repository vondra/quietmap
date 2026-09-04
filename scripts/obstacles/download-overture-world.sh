#!/usr/bin/env bash
# Download the WORLD Overture buildings parquet cache — the one-degree tiles
# scripts/structures/build-structures.py reads its screening stock from.
#
#   scripts/obstacles/download-overture-world.sh [--tiles FILE]
#
# The default comes from the Planet-extracted R4 inventory: every prepared cell
# gets a structures.arrow, so a land cell with no footprint anywhere is
# precisely a tile this job must have fetched. --tiles is the additive recovery
# path for a measured gap. One process fetches every tile
# (download-overture-tiles.py): the theme's parquet footers are read once
# instead of per tile, which is what made the per-tile CLI cost 1-2 GB of
# transfer and 3-4 GB of RAM per tile (2026-09-03).
set -euo pipefail
cd "$(dirname "$0")/../.."

SOURCE_ROOT="data/source/enrichment/global"
PARQUET_DIR="$SOURCE_ROOT/overture-buildings/parquet"
TILE_LIST=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --tiles) TILE_LIST="${2:?need tile list}"; shift 2 ;;
        *) echo "usage: $0 [--tiles FILE]" >&2; exit 2 ;;
    esac
done

python3 -c 'import pyarrow.dataset' 2> /dev/null || { echo "pyarrow missing" >&2; exit 1; }
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

# A tile whose parquet is cached is never re-fetched — the parquet cache itself
# is the resume truth.

total=$(wc -l < "$TILES")
done_n=$(find "$PARQUET_DIR" -maxdepth 1 -name "*.parquet" | wc -l)
echo "[overture-world] $total selected tiles, $done_n cached → $PARQUET_DIR"

fail=0
while :; do
    rc=0
    nice -n 10 python3 scripts/obstacles/download-overture-tiles.py "$TILES" "$PARQUET_DIR" \
        2>> "$PARQUET_DIR/.errors.log" || rc=$?
    [ "$rc" -eq 3 ] && continue   # the fetcher restarts itself at its memory cap; nothing is lost
    [ "$rc" -eq 0 ] || fail=1
    break
done
done_n=$(find "$PARQUET_DIR" -maxdepth 1 -name "*.parquet" | wc -l)
echo "[overture-world] finished: $done_n/$total parquets (fail=$fail)"
exit "$fail"
