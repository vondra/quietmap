#!/usr/bin/env bash
# Convert Copernicus GLO-30 COG tiles → .hgt (i16 BE).
# Copernicus COGs are 3600×3600 pixel-registered 1°×1° tiles.
# Target output is 3601×3601 SRTM-HGT node-registered (pixel centres on
# integer-degree edges). Each node-registered edge row/col needs bilinear
# input from the neighbouring tile, so we warp every tile from a VRT mosaic
# of the whole dataset — warping a single COG leaves its south/east seam
# pixels with no source pixels to interpolate from, producing a 3600-long
# ring of zeros and a visible cross artefact at every integer-degree
# boundary on the heatmap.
#
# Required env: DEM_SRC (downloaded COG tree), DEM_DST (the release .hgt cache).
# Optional: JOBS (default 16). ~30 min with 16 parallel on a full-world run.
set -euo pipefail
cd "$(dirname "$0")/../.."
source scripts/rasters/node-extent.sh

: "${DEM_SRC:?set DEM_SRC to the downloaded copernicus-glo30 tree}"
: "${DEM_DST:?set DEM_DST to the release .hgt cache dir}"
JOBS="${JOBS:-16}"

mkdir -p "$DEM_DST"

# Copernicus tiles are in subdirs: Copernicus_DSM_COG_10_N49_00_E016_00_DEM/
# Each contains a *_DEM.tif file
TILE_LIST="$(mktemp)"
trap 'rm -f "$TILE_LIST" "$VRT"' EXIT
find "$DEM_SRC" -name "*_DEM.tif" -not -name "*_EDM*" > "$TILE_LIST"
TOTAL=$(wc -l < "$TILE_LIST")

# Build a virtual seamless mosaic of every source COG so that per-tile warp
# below can reach across 1°×1° boundaries for its edge samples.
VRT="$(mktemp --suffix=.vrt)"
echo "[cop-dem] $(date '+%H:%M:%S') Building VRT mosaic of $TOTAL source tiles → $VRT"
gdalbuildvrt -q -input_file_list "$TILE_LIST" "$VRT"

echo "[cop-dem] $(date '+%H:%M:%S') Converting $TOTAL Copernicus COG → .hgt ($JOBS parallel)"

convert_one() {
    local tif="$1"
    # Extract tile name from path: Copernicus_DSM_COG_10_N49_00_E016_00_DEM → N49E016
    local dir
    dir=$(basename "$(dirname "$tif")")
    local name
    name=$(echo "$dir" | sed -E 's/Copernicus_DSM_COG_[0-9]+_([NS])([0-9]+)_00_([EW])([0-9]+)_00_DEM/\1\2\3\4/')
    # Pad to standard naming: N49E016
    local ns=${name:0:1}
    local lat=${name:1}
    lat=$(echo "$lat" | sed -E 's/([0-9]+)([EW].*)/\1/')
    local rest=$(echo "$name" | sed -E 's/^[NS][0-9]+//')
    local ew=${rest:0:1}
    local lon=${rest:1}
    local out_name
    out_name=$(printf "%s%02d%s%03d" "$ns" "$((10#$lat))" "$ew" "$((10#$lon))")
    local out="$DEM_DST/${out_name}.hgt"
    [ -f "$out" ] && return 0
    local lat_n=$((10#$lat))
    local lon_n=$((10#$lon))
    [ "$ns" = "S" ] && lat_n=$((-lat_n))
    [ "$ew" = "W" ] && lon_n=$((-lon_n))
    local tmp
    tmp=$(mktemp --suffix=.tif)
    # Warp from the seamless VRT — seam pixels are interpolated across
    # the 1°×1° boundary using the adjacent source tile's data, so the
    # 3601st row/column lands on real elevation, not nodata.
    gdalwarp -q -te $(node_extent $lon_n $lat_n 3601) \
        -ts 3601 3601 -r bilinear -ot Int16 "$VRT" "$tmp" 2>/dev/null || { rm -f "$tmp"; return 0; }
    gdal_translate -of SRTMHGT -q "$tmp" "$out" 2>/dev/null || true
    rm -f "$tmp"
}
export -f convert_one
export DEM_DST VRT

cat "$TILE_LIST" | xargs -P "$JOBS" -I{} bash -c 'convert_one "$@"' _ {}

DONE=$(find "$DEM_DST" -name "*.hgt" | wc -l)
SIZE=$(du -sh "$DEM_DST" 2>/dev/null | cut -f1)
echo "[cop-dem] $(date '+%H:%M:%S') Done: $DONE .hgt tiles, $SIZE"
