#!/usr/bin/env bash
# Convert continuous canopy density into the engine's 1°×1° u8 raster tiles
# (geodata-v2 2a.1 convert half): TCD 2023 (10 m, EPSG:3035, EEA38+UK) where
# available, Hansen GFC treecover2000 masked by lossyear (2000→2024 loss ⇒ 0)
# everywhere else. Output value = mean canopy density 0–100 over each 30 m
# cell (average resampling IS the density semantics), grid 3601×3601 —
# byte-compatible with the binary tree it will replace at the Wave-1 swap.
#
#   scripts/rasters/convert-forest-continuous.sh N50E014 [N49E014 ...]
#   scripts/rasters/convert-forest-continuous.sh --all         # binary-tree census
#   scripts/rasters/convert-forest-continuous.sh --list FILE   # tiles from FILE
#
# Tile conversion parallelizes via xargs; each run owns its own VRT dir.
# Still one instance per tile set — same-tile .raw writes are not atomic.
#
# STAGING output: data/enrichment/global/forest-continuous/<TILE>.raw — the
# live prepared/rasters/forest tree is untouched; Wave 1 swaps the whole tree
# atomically (partial swaps would negative-cache as missing).
set -euo pipefail
cd "$(dirname "$0")/../.."

TCD_DIR="data/source/tcd/2023"
HANSEN_DIR="data/source/hansen/GFC-2024-v1.12"
OUT_DIR="data/enrichment/global/forest-continuous"
CENSUS="data/prepared/rasters/forest"
GRID=3601

mkdir -p "$OUT_DIR"

# VRTs are build scaffolding, not products. They must not sit next to staged
# .raw files: a tree swap that copies OUT_DIR (including hidden names) into
# prepared/rasters/forest/ makes the raster-generation fence fail-closed.
rm -f "$OUT_DIR"/.tcd-2023.vrt "$OUT_DIR"/.hansen-treecover.vrt \
    "$OUT_DIR"/.hansen-lossyear.vrt
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
gdalbuildvrt -q -overwrite "$TC_VRT" "$HANSEN_DIR"/treecover2000/*.tif
gdalbuildvrt -q -overwrite "$LY_VRT" "$HANSEN_DIR"/lossyear/*.tif

convert_one() {
    local tile="$1"
    local out="$OUT_DIR/$tile.raw"
    [ -s "$out" ] && return 0
    local ns="${tile:0:1}" lat="${tile:1:2}" ew="${tile:3:1}" lon="${tile:4:3}"
    lat=$((10#$lat)); lon=$((10#$lon))
    [ "$ns" = "S" ] && lat=$((-lat))
    [ "$ew" = "W" ] && lon=$((-lon))
    local tmp
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' RETURN

    # 1° + one-cell edge alignment like the binary tree: 3601 samples span
    # [lon, lon+1] inclusive ⇒ resolution 1/3600, extent padded half a cell.
    local te
    te="$(python3 -c "
half = 1.0 / 7200.0
print(f'{$lon - half} {$lat - half} {$lon + 1 + half} {$lat + 1 + half}')")"

    # EU lane: TCD where the mosaic has coverage for this tile.
    if [ -s "$TCD_VRT" ]; then
        gdalwarp -q -overwrite -t_srs EPSG:4326 -te $te -ts $GRID $GRID \
            -r average -ot Byte -srcnodata 255 -dstnodata 255 \
            "$TCD_VRT" "$tmp/tcd.tif"
    fi

    # ROW lane: Hansen treecover2000 with post-2000 loss zeroed.
    gdalwarp -q -overwrite -te $te -ts $GRID $GRID -r average -ot Byte \
        "$TC_VRT" "$tmp/tc.tif"
    gdalwarp -q -overwrite -te $te -ts $GRID $GRID -r max -ot Byte \
        "$LY_VRT" "$tmp/ly.tif"

    python3 - "$tmp" "$out" << 'PYEOF'
import sys, numpy as np
from osgeo import gdal
tmp, out = sys.argv[1], sys.argv[2]
def band(path):
    ds = gdal.Open(path)
    return ds.GetRasterBand(1).ReadAsArray().astype(np.uint8) if ds else None
tc = band(f"{tmp}/tc.tif")
ly = band(f"{tmp}/ly.tif")
# Hansen base: canopy 0-100, zeroed where ANY loss year touched the cell
# (max-resampled lossyear > 0). Conservative: a 30 m cell that lost part of
# its trees since 2000 reads as cleared — matches the binary tree's intent.
hansen = np.where(ly > 0, 0, np.minimum(tc, 100)) if tc is not None else None
tcd = band(f"{tmp}/tcd.tif")
if tcd is not None:
    valid = tcd <= 100  # 255 = outside EEA coverage / nodata
    base = hansen if hansen is not None else np.zeros_like(tcd)
    merged = np.where(valid, tcd, base)
else:
    merged = hansen
assert merged is not None, "no source coverage at all"
merged.astype(np.uint8).tofile(out)
PYEOF
    echo "[forest-cont] $tile ok"
}
export -f convert_one
export TCD_VRT TC_VRT LY_VRT OUT_DIR GRID

run_list() {
    local list="$1"
    local total
    total=$(wc -l < "$list")
    echo "[forest-cont] converting $total tiles"
    xargs -P "$(nproc --ignore=8)" -I{} bash -c 'convert_one "$1"' _ {} < "$list"
    echo "[forest-cont] finished: $(ls "$OUT_DIR"/*.raw | wc -l) staged ($total requested)"
}

if [ "${1:-}" = "--all" ]; then
    ls "$CENSUS"/*.raw | sed 's/.*\///; s/\.raw$//' | sort > /tmp/forest-cont-tiles.txt
    run_list /tmp/forest-cont-tiles.txt
elif [ "${1:-}" = "--list" ]; then
    run_list "$2"
else
    for tile in "$@"; do convert_one "$tile"; done
fi
