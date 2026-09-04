#!/usr/bin/env bash
# Cache-fill for the structure height-ladder rasters (scripts/structures/build-structures.py):
#   1. GHS-BUILT-H R2023A ANBH 100 m global GeoTIFF (JRC, CC-BY 4.0, ~2 GB zip)
#      -> data/enrichment/global/ghsl-built-h/
#   2. IPR Praha "Relativni vysky budov" (DSM-DTM, 1 m, F32, S-JTSK, CC BY)
#      -> data/enrichment/2026/cz/ipr-relativni-vysky/tiles/*.tif + mosaic.vrt
#      Praha extent 37.5 x 28 km as 6x7 exportImage tiles of 6250x4000 px;
#      dense-centre tiles that time out server-side fall back to 4 quarters.
# Resumable: existing non-empty artifacts are never re-downloaded.
set -euo pipefail
cd "$(dirname "$0")/../.."

GHSL_DIR="data/enrichment/global/ghsl-built-h"
GHSL_STEM="GHS_BUILT_H_ANBH_E2018_GLOBE_R2023A_54009_100_V1_0"
GHSL_URL="https://jeodpp.jrc.ec.europa.eu/ftp/jrc-opendata/GHSL/GHS_BUILT_H_GLOBE_R2023A/GHS_BUILT_H_ANBH_E2018_GLOBE_R2023A_54009_100/V1-0/$GHSL_STEM.zip"

IPR_DIR="data/enrichment/${DATA_YEAR:-2026}/cz/ipr-relativni-vysky"
IPR_BASE="https://gs-pub.praha.eu/imgs/rest/services/d3m/relativni_vysky_budov/ImageServer/exportImage"
# Service extent (S-JTSK 5514), from the ImageServer metadata.
XMIN=-757500.7777777761
YMIN=-1059999.888888888

mkdir -p "$GHSL_DIR" "$IPR_DIR/tiles"

if [ ! -s "$GHSL_DIR/$GHSL_STEM.tif" ]; then
    if [ ! -s "$GHSL_DIR/$GHSL_STEM.zip" ]; then
        echo "[heights] downloading GHSL ANBH (~2 GB)"
        curl -sSf --max-time 3600 -o "$GHSL_DIR/$GHSL_STEM.zip.part" "$GHSL_URL"
        mv "$GHSL_DIR/$GHSL_STEM.zip.part" "$GHSL_DIR/$GHSL_STEM.zip"
    fi
    # Extract via a temp name + rename — a kill mid-unzip must not leave a
    # truncated final TIFF that resumability trusts forever (gg pass 2).
    unzip -p "$GHSL_DIR/$GHSL_STEM.zip" "$GHSL_STEM.tif" > "$GHSL_DIR/$GHSL_STEM.tif.part"
    mv "$GHSL_DIR/$GHSL_STEM.tif.part" "$GHSL_DIR/$GHSL_STEM.tif"
fi

fetch_ipr() { # bbox-x0 y0 x1 y1 px py out — one exportImage GeoTIFF, verified size
    # Two-step flow (f=json -> href -> download): the direct f=image byte
    # stream times out server-side on dense-centre windows where the href
    # variant succeeds (observed on the two central-Praha tiles, 2026-08-09).
    local x0="$1" y0="$2" x1="$3" y1="$4" px="$5" py="$6" out="$7" attempt href
    for attempt in 1 2 3 4; do
        href=$(curl -sf --max-time 300 \
            "$IPR_BASE?bbox=$x0,$y0,$x1,$y1&bboxSR=5514&imageSR=5514&size=$px,$py&format=tiff&pixelType=F32&interpolation=RSP_NearestNeighbor&f=json" \
            | python3 -c 'import json,sys; print(json.load(sys.stdin).get("href",""))' 2> /dev/null) || href=""
        if [ -n "$href" ] \
            && curl -sf --max-time 300 -o "$out.dl" "$href" \
            && gdalinfo "$out.dl" 2> /dev/null | grep -q "Size is $px, $py"; then
            mv "$out.dl" "$out"
            return 0
        fi
        sleep $((attempt * 8))
    done
    rm -f "$out.dl"
    return 1
}

for col in 0 1 2 3 4 5; do
    for row in 0 1 2 3 4 5 6; do
        name="c${col}r${row}"
        out="$IPR_DIR/tiles/$name.tif"
        [ -s "$out" ] && continue
        x0=$(python3 -c "print($XMIN + $col*6250)")
        y0=$(python3 -c "print($YMIN + $row*4000)")
        x1=$(python3 -c "print($XMIN + ($col+1)*6250)")
        y1=$(python3 -c "print($YMIN + ($row+1)*4000)")
        # -a_srs: exportImage TIFFs carry a datum-less LOCAL_CS shell, not a
        # transformable EPSG:5514 (gg 2026-08-09) — stamp the real CRS here.
        # Finals are only ever produced by rename (gg pass 2): gdal writes to
        # a .part sibling, so a kill/ENOSPC cannot leave a truncated final
        # that the resumability check ([-s]) would then skip forever.
        if fetch_ipr "$x0" "$y0" "$x1" "$y1" 6250 4000 "$out.full"; then
            gdal_translate -q -a_srs EPSG:5514 -co COMPRESS=DEFLATE -co TILED=YES "$out.full" "$out.part"
            mv "$out.part" "$out"
            rm -f "$out.full"
            echo "[heights] IPR $name"
            continue
        fi
        # Dense-centre tiles exceed the server's compute window — quarters.
        parts=()
        for qx in 0 1; do for qy in 0 1; do
            qx0=$(python3 -c "print($x0 + $qx*3125)")
            qy0=$(python3 -c "print($y0 + $qy*2000)")
            qx1=$(python3 -c "print($x0 + ($qx+1)*3125)")
            qy1=$(python3 -c "print($y0 + ($qy+1)*2000)")
            q="$IPR_DIR/tiles/.${name}_q$qx$qy.tif"
            fetch_ipr "$qx0" "$qy0" "$qx1" "$qy1" 3125 2000 "$q" || { echo "[heights] IPR $name FAILED" >&2; exit 1; }
            parts+=("$q")
        done; done
        for p in "${parts[@]}"; do python3 -c "
from osgeo import gdal, osr
gdal.UseExceptions()
srs = osr.SpatialReference(); srs.ImportFromEPSG(5514)
ds = gdal.Open('$p', gdal.GA_Update); ds.SetProjection(srs.ExportToWkt())"; done
        gdalwarp -q -overwrite -co COMPRESS=DEFLATE -co TILED=YES "${parts[@]}" "$out.part"
        mv "$out.part" "$out"
        rm -f "${parts[@]}"
        echo "[heights] IPR $name (quarters)"
    done
done

gdalbuildvrt -q -overwrite "$IPR_DIR/mosaic.vrt" "$IPR_DIR"/tiles/c*.tif

# Cache validation (gg 2026-08-09): every tile must open, carry EPSG:5514 and
# the exact grid; the mosaic must span the full tiling extent; the Žižkov TV
# tower (50.0810 N, 14.4510 E) must stand tall — any CRS/georeference/partial-
# file regression fails HERE, before any enrichment reads.
python3 - "$IPR_DIR" <<'PYEOF'
import glob, sys
from osgeo import gdal
from pyproj import CRS, Transformer
gdal.UseExceptions()
tiles = sorted(glob.glob(sys.argv[1] + "/tiles/c*.tif"))
assert len(tiles) == 42, f"expected 42 tiles, found {len(tiles)}"
for t in tiles:
    d = gdal.Open(t)
    assert (d.RasterXSize, d.RasterYSize) == (6250, 4000), f"{t}: wrong grid"
    assert CRS.from_wkt(d.GetProjection()).to_epsg() == 5514, f"{t}: CRS not EPSG:5514"
    assert d.GetRasterBand(1).ReadAsArray(0, 0, 1, 1) is not None, f"{t}: unreadable"
ds = gdal.Open(sys.argv[1] + "/mosaic.vrt")
assert (ds.RasterXSize, ds.RasterYSize) == (37500, 28000), "mosaic does not span the 6x7 tiling extent"
crs = CRS.from_wkt(ds.GetProjection())
assert crs.to_epsg() == 5514, f"mosaic CRS is {crs.name!r}, not EPSG:5514"
tr = Transformer.from_crs("EPSG:4326", crs, always_xy=True)
x, y = tr.transform(14.4510, 50.0810)
gt = ds.GetGeoTransform()
ci, ri = int((x - gt[0]) / gt[1]), int((y - gt[3]) / gt[5])
w = ds.GetRasterBand(1).ReadAsArray(ci - 40, ri - 40, 80, 80)
assert w is not None and 150.0 <= float(w.max()) <= 230.0, f"Žižkov tower control point reads {w is not None and w.max()} m"
print(f"[heights] cache validated: 42 tiles EPSG:5514, full extent, Žižkov tower {w.max():.1f} m")
PYEOF
echo "[heights] caches ready: $GHSL_DIR/$GHSL_STEM.tif + $IPR_DIR/mosaic.vrt"
