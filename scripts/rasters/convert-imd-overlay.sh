#!/usr/bin/env bash
# Overlay Copernicus IMD (Europe, continuous 0-100%) over the WorldCover-derived
# IMD proxy. Overwrites existing .raw tiles where Copernicus data is available.
#
# Required env: IMD_SRC_ROOT (release imd source, year subdirs), IMD_DST
# (release rasters/imd).
set -euo pipefail
cd "$(dirname "$0")/../.."
source scripts/rasters/node-extent.sh
QM_VENV_PYTHON="${QM_VENV_PYTHON:-$PWD/.venv/bin/python}"

: "${IMD_SRC_ROOT:?set IMD_SRC_ROOT to the release imd source dir}"
: "${IMD_DST:?set IMD_DST to the release rasters/imd dir}"

# Find all available Copernicus IMD source files
IMD_SOURCES=()
# Ascending years: gdalbuildvrt paints later files over earlier, so the
# newest release wins where coverage overlaps.
for f in "$IMD_SRC_ROOT"/2018/*.tif "$IMD_SRC_ROOT"/2021/*.tif "$IMD_SRC_ROOT"/2024/*.tif; do
    [ -f "$f" ] && IMD_SOURCES+=("$f")
done

if [ ${#IMD_SOURCES[@]} -eq 0 ]; then
    echo "[imd-overlay] No Copernicus IMD source files found. Skipping."
    exit 0
fi

echo "[imd-overlay] $(date '+%H:%M:%S') Overlaying ${#IMD_SOURCES[@]} Copernicus IMD file(s)"

VRT_DIR=$(mktemp -d "${TMPDIR:-/tmp}/imd-overlay-vrt.XXXXXX")
trap 'rm -rf "$VRT_DIR"' EXIT

# Build VRT if multiple sources
if [ ${#IMD_SOURCES[@]} -eq 1 ]; then
    SRC="${IMD_SOURCES[0]}"
else
    SRC="$VRT_DIR/imd_copernicus.vrt"
    gdalbuildvrt -q "$SRC" "${IMD_SOURCES[@]}"
fi

# Get extent of source data. MUST come from wgs84Extent, not cornerCoordinates:
# corners are in the source SRS — for an EPSG:3857 source they are metres
# (lat "6830000"), and feeding those to the seq loops below means ~5×10^11
# iterations = a silent permanent hang (/gg Gemini 2026-06-11, verified).
EXTENT=$(gdalinfo "$SRC" -json 2>/dev/null | python3 -c "
import sys, json, math
info = json.load(sys.stdin)
ring = info.get('wgs84Extent', {}).get('coordinates', [[]])[0]
if not ring:
    sys.exit('no wgs84Extent in gdalinfo output — cannot derive tile range')
lats = [p[1] for p in ring]
lons = [p[0] for p in ring]
print(f'{math.floor(min(lats))} {math.floor(max(lats))} '
      f'{math.floor(min(lons))} {math.floor(max(lons))}')
")
read LAT_MIN LAT_MAX LON_MIN LON_MAX <<< "$EXTENT"
echo "[imd-overlay] $(date '+%H:%M:%S') Extent: lat $LAT_MIN..$LAT_MAX, lon $LON_MIN..$LON_MAX"

COUNT=0
FAILED=0
for lat in $(seq "$LAT_MIN" "$LAT_MAX"); do
    for lon in $(seq "$LON_MIN" "$LON_MAX"); do
        ns="N"; [ "$lat" -lt 0 ] && ns="S"
        ew="E"; [ "$lon" -lt 0 ] && ew="W"
        NAME=$(printf "%s%02d%s%03d" "$ns" "${lat#-}" "$ew" "${lon#-}")
        DST="$IMD_DST/${NAME}.raw"

        TMP="$VRT_DIR/imd_overlay_${NAME}.tif"
        rm -f "$TMP"   # leftover from an interrupted run makes gdalwarp fail
        gdalwarp -q -t_srs EPSG:4326 \
            -te $(node_extent $lon $lat 3601) \
            -ts 3601 3601 -r bilinear -ot Byte \
            "$SRC" "$TMP" 2>/dev/null || continue

        # tmp + os.replace: live popup readers must never see a torn tile
        DST="$DST" TMP="$TMP" "$QM_VENV_PYTHON" << 'PYEOF' || { FAILED=$((FAILED + 1)); echo "[imd-overlay] WARN: reclassify failed" >&2; }
import numpy as np, os
import rasterio
tmp, dst = os.environ["TMP"], os.environ["DST"]
with rasterio.open(tmp) as ds:
    arr = ds.read(1)
arr = np.clip(arr, 0, 100).astype(np.uint8)
if np.any(arr > 0):
    # Copernicus HRL maps water as 0% impervious -> G=1 soft, re-breaking the
    # ISO water=hard fix. In the WorldCover-derived base, exactly value 100 is
    # water (built-up is 85) -- preserve it where Copernicus says 0.
    if os.path.exists(dst):
        base = np.fromfile(dst, dtype=np.uint8).reshape(arr.shape)
        arr = np.where((arr == 0) & (base == 100), np.uint8(100), arr)
    tmp_out = dst + '.tmp.' + str(os.getpid())
    arr.tofile(tmp_out)
    os.replace(tmp_out, dst)
PYEOF
        rm -f "$TMP"
        COUNT=$((COUNT + 1))
    done
done

echo "[imd-overlay] $(date '+%H:%M:%S') Done: $COUNT tiles overwritten with Copernicus IMD"
if [ "$FAILED" -gt 0 ]; then
    echo "[imd-overlay] ERROR: $FAILED tile(s) failed to reclassify" >&2
    exit 1
fi
