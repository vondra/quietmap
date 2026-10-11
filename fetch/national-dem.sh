#!/usr/bin/env bash
# Prepares a national terrain model for `qm-build`: the source tiles (already downloaded, one CRS
# per group) mosaicked onto pixels centred on the nodes of a lattice of NODES_PER_DEGREE (3600: the
# one arc-second terrain, `--national-dem`; 21600: the fine terrain near a ray's ends, `fine-terrain
# --fine-dem`), heights moved to EGM2008 by PROJ with the geoid grids (never a ballpark: the grids
# must be on PROJ_DATA), float32, no data -9999. RESAMPLING is gdalwarp's: `average` where a node's
# pixel spans many source pixels (5 m onto 1"), `bilinear` where it spans about one (5 m onto 1/6").
#
#   national-dem.sh OUT.tif GRIDS_DIR NODES_PER_DEGREE RESAMPLING WEST SOUTH EAST NORTH SOURCE_SRS:TILE_LIST ..
#
# WEST..NORTH are whole node indices (degrees x NODES_PER_DEGREE); SOURCE_SRS is a compound CRS
# such as EPSG:5514+8357 (CZ DMR5G: S-JTSK, Bpv) or EPSG:25832+7837 (DE: UTM 32N, DHHN2016); each
# tile list holds the files of one CRS. Examples (the areas the r051 terrain lays over its base):
#   CZ: national-dem.sh cz.tif GRIDS 3600 average 43200 174600 68040 183960 EPSG:5514+8357:dmr5g-tiles.txt
#   DE: national-dem.sh de.tif GRIDS 3600 average 20880 169920 54360 198360 \
#         EPSG:25832+7837:de-25832.txt EPSG:25833+7837:de-25833.txt
set -euo pipefail
out=$1 grids=$2 nodes=$3 resampling=$4 west=$5 south=$6 east=$7 north=$8
shift 8
export PROJ_DATA="/usr/share/proj:$grids" PROJ_NETWORK=OFF GTIFF_SRS_SOURCE=EPSG
edge() { python3 -c "print(repr(($1 + $2) / $nodes))"; }
vrts=()
for source in "$@"; do
    srs=${source%:*} list=${source##*:}
    vrt="${out%.tif}.${srs//[:+]/-}.vrt"
    gdalbuildvrt -q -a_srs "$srs" -input_file_list "$list" "$vrt"
    vrts+=("$vrt")
done
step=$(python3 -c "print(repr(1 / $nodes))")
gdalwarp -q -t_srs EPSG:4326+3855 -r "$resampling" -tr "$step" "$step" \
    -te "$(edge "$west" -0.5)" "$(edge "$south" -0.5)" "$(edge "$east" 0.5)" "$(edge "$north" 0.5)" \
    -ot Float32 -dstnodata -9999 \
    -co COMPRESS=DEFLATE -co PREDICTOR=3 -co TILED=YES -co BIGTIFF=YES \
    -multi -wo NUM_THREADS=8 -wm 2048 "${vrts[@]}" "$out"
