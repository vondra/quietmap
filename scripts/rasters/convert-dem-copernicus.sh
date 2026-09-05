#!/usr/bin/env bash
# Build complete node-registered Copernicus intermediates as big-endian i16 HGT.
# Copernicus longitude spacing varies by latitude. Native band mosaics retain
# source pixels and wrap ±180° before exact node interpolation to 3601² HGT.
#
# Required env: DEM_SRC (catalog.sqlite + glo30/glo90), DEM_DST (HGT cache).
# --coverage-gaps rebuilds only catalog-derived missing land and shared-edge neighbors.
set -euo pipefail
cd "$(dirname "$0")/../.."

: "${DEM_SRC:?set DEM_SRC to the complete Copernicus source root}"
: "${DEM_DST:?set DEM_DST to the release .hgt cache dir}"
JOBS="${JOBS:-16}"
GRID=3601
EXPECTED_BYTES=$((2 * GRID * GRID))

mkdir -p "$DEM_DST"

VRT_DIR=$(mktemp -d "${TMPDIR:-/tmp}/cop-dem-vrt.XXXXXX")
trap 'rm -rf -- "$VRT_DIR"' EXIT
python3 scripts/rasters/dem_mosaic.py "$DEM_SRC" "$VRT_DIR" "$@"
TILE_LIST="$VRT_DIR/tiles.txt"
TOTAL=$(wc -l < "$TILE_LIST")
REBUILD_TILES=" $(tr '\n' ' ' < "$VRT_DIR/changed.txt") "

echo "[cop-dem] $(date '+%H:%M:%S') Converting $TOTAL Copernicus COG → .hgt ($JOBS parallel)"

convert_one() (
    local out_name="$1"
    if [[ ! "$out_name" =~ ^([NS])([0-9]{2})([EW])([0-9]{3})$ ]]; then
        echo "[cop-dem] invalid catalog tile: $out_name" >&2
        return 2
    fi
    local ns="${BASH_REMATCH[1]}" lat="${BASH_REMATCH[2]}"
    local ew="${BASH_REMATCH[3]}" lon="${BASH_REMATCH[4]}"
    local out="$DEM_DST/${out_name}.hgt"
    if [ -f "$out" ]; then
        local current_bytes
        current_bytes=$(stat -c%s "$out")
        if [ "$current_bytes" -eq "$EXPECTED_BYTES" ] && [[ "$REBUILD_TILES" != *" $out_name "* ]]; then
            return 0
        fi
    fi
    local lat_n=$((10#$lat))
    local lon_n=$((10#$lon))
    [ "$ns" = "S" ] && lat_n=$((-lat_n))
    [ "$ew" = "W" ] && lon_n=$((-lon_n))
    local staged_dir
    trap '[ -z "${staged_dir:-}" ] || rm -rf -- "$staged_dir"' EXIT
    staged_dir=$(mktemp -d "$DEM_DST/.${out_name}.XXXXXX")
    local staged_out="$staged_dir/${out_name}.hgt"
    python3 scripts/rasters/dem_native_grid.py "$VRT_DIR" "$lon_n" "$lat_n" "$staged_out"
    local actual_bytes
    actual_bytes=$(stat -c%s "$staged_out")
    if [ "$actual_bytes" -ne "$EXPECTED_BYTES" ]; then
        echo "[cop-dem] $out_name produced $actual_bytes bytes, expected $EXPECTED_BYTES" >&2
        return 1
    fi
    sync -d "$staged_out"
    chmod 0664 "$staged_out"
    mv -f -- "$staged_out" "$out"
)
export -f convert_one
export DEM_DST VRT_DIR EXPECTED_BYTES REBUILD_TILES

# shellcheck disable=SC2016
xargs -r -P "$JOBS" -I{} bash -euo pipefail -c 'convert_one "$1"' _ {} < "$TILE_LIST"

SIZE=$(du -sh "$DEM_DST" 2>/dev/null | cut -f1)
echo "[cop-dem] $(date '+%H:%M:%S') Done: $TOTAL selected .hgt tiles, $SIZE"
