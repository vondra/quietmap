#!/usr/bin/env bash
# Download the WORLD Overture buildings parquet cache — the one-degree tiles
# build-structures reads its screening stock from.
#
#   OVERTURE_PARQUET_DIR=/data/.../source/2026/overture/parquet \
#     scripts/overture/download-overture-world.sh [--tiles FILE]
#
# The tile list defaults to <parquet-dir>/../tiles.txt (built once from the
# frozen R4 inventory — a migration bridge, not product code); --tiles is the
# additive recovery path for a measured gap. The parquet cache itself is the
# only resume truth: cached tiles are never re-fetched. A restart exit (3)
# means relaunch the same command; a nonzero tile count means inspect the log.
set -euo pipefail

: "${OVERTURE_PARQUET_DIR:?set OVERTURE_PARQUET_DIR to the release overture parquet dir}"
TILE_LIST=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --tiles) TILE_LIST="${2:?need tile list}"; shift 2 ;;
        *) echo "usage: $0 [--tiles FILE]" >&2; exit 2 ;;
    esac
done

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
VENV="$ROOT/.venv/bin/python"
# Lexical (no filesystem traversal): the parquet dir may not exist yet.
TILES="$(dirname "$OVERTURE_PARQUET_DIR")/tiles.txt"
[ -n "$TILE_LIST" ] && TILES="$TILE_LIST"
mkdir -p "$OVERTURE_PARQUET_DIR"
[ -s "$TILES" ] || { echo "tile list missing or empty: $TILES" >&2; exit 2; }

total=$(wc -l < "$TILES")
done_n=$(find "$OVERTURE_PARQUET_DIR" -maxdepth 1 -name "*.parquet" | wc -l)
echo "[overture-world] $total selected tiles, $done_n cached → $OVERTURE_PARQUET_DIR"

fail=0
while :; do
    set +e
    "$VENV" "$ROOT/scripts/overture/download-overture-tiles.py" "$TILES" "$OVERTURE_PARQUET_DIR"
    code=$?
    set -e
    if [ "$code" -eq 0 ]; then break; fi
    if [ "$code" -eq 3 ]; then
        echo "[overture-world] worker asked for a fresh process; relaunching ..." >&2
        continue
    fi
    fail=1
    break
done
[ "$fail" -eq 0 ]
