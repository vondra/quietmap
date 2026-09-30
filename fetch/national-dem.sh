#!/usr/bin/env bash
# Prepares a national terrain model for `qm-build --national-dem`: the source tiles (already
# downloaded, one CRS per group) mosaicked, area-averaged onto one arc-second pixels centred on the
# lattice nodes, heights moved to EGM2008 by PROJ with the geoid grids (never a ballpark: the grids
# must be on PROJ_DATA), float32, no data -9999.
#
#   national-dem.sh OUT.tif GRIDS_DIR WEST SOUTH EAST NORTH SOURCE_SRS:TILE_LIST [SOURCE_SRS:TILE_LIST ..]
#
# WEST..NORTH are whole node indices (degrees x 3600); SOURCE_SRS is a compound CRS such as
# EPSG:5514+8357 (CZ DMR5G: S-JTSK, Bpv) or EPSG:25832+7837 (DE: UTM 32N, DHHN2016); each tile list
# holds the files of one CRS. Examples (the areas the r051 terrain lays over its base):
#   CZ: national-dem.sh cz.tif GRIDS 43200 174600 68040 183960 EPSG:5514+8357:dmr5g-tiles.txt
#   DE: national-dem.sh de.tif GRIDS 20880 169920 54360 198360 EPSG:25832+7837:de-25832.txt \
#         EPSG:25833+7837:de-25833.txt
set -euo pipefail
out=$1 grids=$2 west=$3 south=$4 east=$5 north=$6
shift 6
export PROJ_DATA="/usr/share/proj:$grids" PROJ_NETWORK=OFF GTIFF_SRS_SOURCE=EPSG
edge() { python3 -c "print(repr(($1 + $2) / 3600))"; }
vrts=()
for source in "$@"; do
    srs=${source%:*} list=${source##*:}
    vrt="${out%.tif}.${srs//[:+]/-}.vrt"
    gdalbuildvrt -q -a_srs "$srs" -input_file_list "$list" "$vrt"
    vrts+=("$vrt")
done
gdalwarp -q -t_srs EPSG:4326+3855 -r average \
    -tr 0.000277777777777777777 0.000277777777777777777 \
    -te "$(edge "$west" -0.5)" "$(edge "$south" -0.5)" "$(edge "$east" 0.5)" "$(edge "$north" 0.5)" \
    -ot Float32 -dstnodata -9999 \
    -co COMPRESS=DEFLATE -co PREDICTOR=3 -co TILED=YES -co BIGTIFF=YES \
    -multi -wo NUM_THREADS=8 -wm 2048 "${vrts[@]}" "$out"
