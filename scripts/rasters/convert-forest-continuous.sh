#!/usr/bin/env bash
# Convert continuous canopy density → 1°×1° u8 forest tiles (3601×3601, 30 m):
# TCD 2023 (10 m, EEA38+UK) where available, Hansen GFC treecover2000 masked
# by lossyear (2000→2024 loss ⇒ 0) everywhere else. Output value = mean canopy
# density 0–100 per 30 m cell (average resampling IS the density semantics).
#
#   scripts/rasters/convert-forest-continuous.sh N50E014 [N49E014 ...]
#   scripts/rasters/convert-forest-continuous.sh --all         # from the tile list
#   scripts/rasters/convert-forest-continuous.sh --list FILE   # tiles from FILE
#
# Required env: TCD_DIR (may be empty dir), HANSEN_DIR, FOREST_DST (release
# rasters/forest), TILE_LIST (for --all). A tile becomes visible only after its
# exact-size output has been flushed and atomically renamed into place.
set -euo pipefail
cd "$(dirname "$0")/../.."
source scripts/rasters/node-extent.sh
QM_VENV_PYTHON="${QM_VENV_PYTHON:-$PWD/.venv/bin/python}"

: "${TCD_DIR:?set TCD_DIR to the release tcd source dir}"
: "${HANSEN_DIR:?set HANSEN_DIR to the release hansen source dir}"
: "${FOREST_DST:?set FOREST_DST to the release rasters/forest dir}"
GRID=3601
EXPECTED_BYTES=$((GRID * GRID))

mkdir -p "$FOREST_DST"

# VRTs are build scaffolding in a private temp dir, never next to products.
VRT_DIR=$(mktemp -d "${TMPDIR:-/tmp}/forest-cont-vrt.XXXXXX")
trap 'rm -rf "$VRT_DIR"' EXIT
TCD_VRT="$VRT_DIR/tcd-2023.vrt"
if ls "$TCD_DIR"/*.tif &> /dev/null; then
    # -vrtnodata 255: tiles not (yet) downloaded must read as NODATA in the
    # mosaic gaps, never as 0 % canopy — 0 is a real value inside coverage.
    gdalbuildvrt -q -overwrite -vrtnodata 255 -srcnodata 255 "$TCD_VRT" "$TCD_DIR"/*.tif
fi
TC_VRT="$VRT_DIR/hansen-treecover.vrt"
LY_VRT="$VRT_DIR/hansen-lossyear.vrt"
TC_LIST="$VRT_DIR/hansen-treecover.txt"
LY_LIST="$VRT_DIR/hansen-lossyear.txt"
"$QM_VENV_PYTHON" scripts/rasters/hansen_sources.py \
    "$HANSEN_DIR" "$TC_LIST" "$LY_LIST"
gdalbuildvrt -q -strict -overwrite -input_file_list "$TC_LIST" "$TC_VRT"
# Published NoSuchKey lossyear granules mean an all-zero loss mask. Omitting
# only those validated markers from this full-extent mosaic yields exactly 0
# in their gaps; every other unreadable source has already failed validation.
gdalbuildvrt -q -strict -overwrite -input_file_list "$LY_LIST" "$LY_VRT"

convert_one() (
    local tile="$1"
    if [[ ! "$tile" =~ ^[NS][0-9]{2}[EW][0-9]{3}$ ]]; then
        echo "[forest-cont] invalid tile name: $tile" >&2
        return 2
    fi
    local out="$FOREST_DST/$tile.raw"
    if [ -f "$out" ]; then
        local current_bytes
        current_bytes=$(stat -c%s "$out")
        [ "$current_bytes" -eq "$EXPECTED_BYTES" ] && return 0
        echo "[forest-cont] $tile rebuilding incomplete $current_bytes-byte output" >&2
    fi
    local ns="${tile:0:1}" lat="${tile:1:2}" ew="${tile:3:1}" lon="${tile:4:3}"
    lat=$((10#$lat)); lon=$((10#$lon))
    [ "$ns" = "S" ] && lat=$((-lat))
    [ "$ew" = "W" ] && lon=$((-lon))
    local tmp staged_out
    tmp=$(mktemp -d "${TMPDIR:-/tmp}/forest-cont-tile.XXXXXX")
    trap 'rm -rf -- "$tmp"; [ -z "${staged_out:-}" ] || rm -f -- "$staged_out"' EXIT
    staged_out=$(mktemp "$FOREST_DST/.${tile}.raw.XXXXXX")

    # 1° + one-cell edge alignment: 3601 samples span [lon, lon+1] inclusive
    # ⇒ resolution 1/3600, extent padded half a cell.
    local -a tile_extent
    read -r -a tile_extent <<< "$(node_extent "$lon" "$lat" "$GRID")"

    # EU lane: TCD where the mosaic has coverage for this tile.
    if [ -s "$TCD_VRT" ]; then
        gdalwarp -q -overwrite -t_srs EPSG:4326 -te "${tile_extent[@]}" -ts "$GRID" "$GRID" \
            -r average -ovr NONE -ot Byte -srcnodata 255 -dstnodata 255 \
            "$TCD_VRT" "$tmp/tcd.tif"
    fi

    # ROW lane: Hansen treecover2000 with post-2000 loss zeroed.
    gdalwarp -q -overwrite -te "${tile_extent[@]}" -ts "$GRID" "$GRID" -r average -ot Byte \
        "$TC_VRT" "$tmp/tc.tif"
    gdalwarp -q -overwrite -te "${tile_extent[@]}" -ts "$GRID" "$GRID" -r max -ot Byte \
        "$LY_VRT" "$tmp/ly.tif"

    "$QM_VENV_PYTHON" - "$tmp" "$staged_out" "$GRID" << 'PYEOF'
import os
from pathlib import Path
import sys

import numpy as np
import rasterio

tmp, out, grid_text = sys.argv[1:]
expected_shape = (int(grid_text), int(grid_text))

def band(path, *, required):
    if not Path(path).is_file():
        if required:
            raise RuntimeError(f"required warped raster is missing: {path}")
        return None
    with rasterio.open(path) as dataset:
        values = dataset.read(1)
    if values.shape != expected_shape:
        raise RuntimeError(f"wrong warped raster shape for {path}: {values.shape}")
    return values.astype(np.uint8, copy=False)

tc = band(f"{tmp}/tc.tif", required=True)
ly = band(f"{tmp}/ly.tif", required=True)
# Hansen base: canopy 0-100, zeroed where ANY loss year touched the cell
# (max-resampled lossyear > 0). Conservative: a 30 m cell that lost part of
# its trees since 2000 reads as cleared.
hansen = np.where(ly > 0, 0, np.minimum(tc, 100))
tcd = band(f"{tmp}/tcd.tif", required=False)
if tcd is not None:
    valid = tcd <= 100  # 255 = outside EEA coverage / nodata
    merged = np.where(valid, tcd, hansen)
else:
    merged = hansen
with open(out, "wb") as destination:
    merged.astype(np.uint8, copy=False).tofile(destination)
    destination.flush()
    os.fsync(destination.fileno())
PYEOF
    local actual_bytes
    actual_bytes=$(stat -c%s "$staged_out")
    if [ "$actual_bytes" -ne "$EXPECTED_BYTES" ]; then
        echo "[forest-cont] $tile produced $actual_bytes bytes, expected $EXPECTED_BYTES" >&2
        return 1
    fi
    chmod 0664 "$staged_out"
    mv -f -- "$staged_out" "$out"
    staged_out=""
    echo "[forest-cont] $tile ok"
)
export -f convert_one
export TCD_VRT TC_VRT LY_VRT FOREST_DST GRID EXPECTED_BYTES QM_VENV_PYTHON

run_list() {
    local list="$1"
    local total
    total=$(wc -l < "$list")
    echo "[forest-cont] converting $total tiles"
    # The quoted $1 must expand inside each child shell, not in this owner.
    # shellcheck disable=SC2016
    xargs -r -P "$(nproc --ignore=8)" -I{} \
        bash -euo pipefail -c 'convert_one "$1"' _ {} < "$list"
    echo "[forest-cont] finished: $(find "$FOREST_DST" -maxdepth 1 -name '*.raw' | wc -l) staged ($total requested)"
}

if [ "${1:-}" = "--all" ]; then
    : "${TILE_LIST:?set TILE_LIST to the release land-tile list}"
    run_list "$TILE_LIST"
elif [ "${1:-}" = "--list" ]; then
    run_list "$2"
else
    for tile in "$@"; do convert_one "$tile"; done
fi
