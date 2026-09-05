#!/usr/bin/env bash
# Build complete node-registered Copernicus intermediates as big-endian i16 HGT.
# Copernicus COGs are 3600×3600 pixel-registered 1°×1° tiles.
# Target output is 3601×3601 SRTM-HGT node-registered (pixel centres on
# integer-degree edges). Each node-registered edge row/col needs bilinear
# input from the neighbouring tile, so we warp every tile from a VRT mosaic
# of the whole dataset — warping a single COG leaves its south/east seam
# pixels with no source pixels to interpolate from, producing a 3600-long
# ring of zeros at every integer-degree boundary.
#
# Required env: DEM_SRC (downloaded COG tree), DEM_DST (the release .hgt cache).
# Optional: JOBS (default 16). Failures preserve existing outputs and fail the run.
set -euo pipefail
cd "$(dirname "$0")/../.."
source scripts/rasters/node-extent.sh

: "${DEM_SRC:?set DEM_SRC to the downloaded copernicus-glo30 tree}"
: "${DEM_DST:?set DEM_DST to the release .hgt cache dir}"
JOBS="${JOBS:-16}"
GRID=3601
EXPECTED_BYTES=$((2 * GRID * GRID))

mkdir -p "$DEM_DST"

# Copernicus tiles are in subdirs: Copernicus_DSM_COG_10_N49_00_E016_00_DEM/
# Each contains a *_DEM.tif file
VRT_DIR=$(mktemp -d "${TMPDIR:-/tmp}/cop-dem-vrt.XXXXXX")
trap 'rm -rf -- "$VRT_DIR"' EXIT
TILE_LIST="$VRT_DIR/sources.txt"
VRT="$VRT_DIR/mosaic.vrt"
find "$DEM_SRC" -type f -name "*_DEM.tif" > "$TILE_LIST"
TOTAL=$(wc -l < "$TILE_LIST")
if [ "$TOTAL" -eq 0 ]; then
    echo "[cop-dem] no Copernicus source tiles in $DEM_SRC" >&2
    exit 2
fi

# Build a virtual seamless mosaic of every source COG so that per-tile warp
# below can reach across 1°×1° boundaries for its edge samples.
echo "[cop-dem] $(date '+%H:%M:%S') Building VRT mosaic of $TOTAL source tiles → $VRT"
gdalbuildvrt -q -strict -input_file_list "$TILE_LIST" "$VRT"

echo "[cop-dem] $(date '+%H:%M:%S') Converting $TOTAL Copernicus COG → .hgt ($JOBS parallel)"

convert_one() (
    local tif="$1" dir
    dir="${tif%/*}"
    dir="${dir##*/}"
    if [[ ! "$dir" =~ ^Copernicus_DSM_COG_[0-9]+_([NS])([0-9]{2})_00_([EW])([0-9]{3})_00_DEM$ ]]; then
        echo "[cop-dem] invalid Copernicus tile path: $tif" >&2
        return 2
    fi
    local ns="${BASH_REMATCH[1]}" lat="${BASH_REMATCH[2]}"
    local ew="${BASH_REMATCH[3]}" lon="${BASH_REMATCH[4]}"
    local out_name="${ns}${lat}${ew}${lon}"
    local out="$DEM_DST/${out_name}.hgt"
    if [ -f "$out" ]; then
        local current_bytes
        current_bytes=$(stat -c%s "$out")
        [ "$current_bytes" -eq "$EXPECTED_BYTES" ] && return 0
        echo "[cop-dem] $out_name rebuilding incomplete $current_bytes-byte output" >&2
    fi
    local lat_n=$((10#$lat))
    local lon_n=$((10#$lon))
    [ "$ns" = "S" ] && lat_n=$((-lat_n))
    [ "$ew" = "W" ] && lon_n=$((-lon_n))
    local tmp staged_dir
    tmp=$(mktemp -d "${TMPDIR:-/tmp}/cop-dem-tile.XXXXXX")
    trap 'rm -rf -- "$tmp"; [ -z "${staged_dir:-}" ] || rm -rf -- "$staged_dir"' EXIT
    # SRTMHGT requires the canonical basename; keep it in a same-filesystem stage.
    staged_dir=$(mktemp -d "$DEM_DST/.${out_name}.XXXXXX")
    local staged_out="$staged_dir/${out_name}.hgt"
    local -a tile_extent
    read -r -a tile_extent <<< "$(node_extent "$lon_n" "$lat_n" "$GRID")"
    # Warp from the seamless VRT — seam pixels are interpolated across
    # the 1°×1° boundary using the adjacent source tile's data, so the
    # 3601st row/column lands on real elevation, not nodata.
    gdalwarp -q -te "${tile_extent[@]}" -ts "$GRID" "$GRID" -r bilinear -ot Int16 \
        "$VRT" "$tmp/warped.tif"
    GDAL_PAM_ENABLED=NO gdal_translate -of SRTMHGT -q "$tmp/warped.tif" "$staged_out"
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
export DEM_DST VRT GRID EXPECTED_BYTES

# shellcheck disable=SC2016
xargs -r -P "$JOBS" -I{} bash -euo pipefail -c 'convert_one "$1"' _ {} < "$TILE_LIST"

DONE=$(find "$DEM_DST" -maxdepth 1 -type f -name "*.hgt" | wc -l)
if [ "$DONE" -ne "$TOTAL" ]; then
    echo "[cop-dem] output inventory differs: $DONE HGT tiles for $TOTAL sources" >&2
    exit 1
fi
SIZE=$(du -sh "$DEM_DST" 2>/dev/null | cut -f1)
echo "[cop-dem] $(date '+%H:%M:%S') Done: $DONE .hgt tiles, $SIZE"
