#!/usr/bin/env bash
# The places of worship, bell towers and minarets of an OpenStreetMap planet file (ODbL) for the
# bells and calls to prayer: the features alone (osmium tags-filter), then each as a point with
# its religion, kind, mapped height and denomination (`qm-build dev4 --kinds sources --worship
# OUT_DIR/worship.txt`). Needs osmium-tool and Python 3.
#
#   worship.sh PLANET.osm.pbf OUT_DIR
set -euo pipefail
planet=$1 out=$2
mkdir -p "$out"
osmium tags-filter "$planet" nwr/amenity=place_of_worship nwr/tower:type=bell_tower,minaret \
    nwr/building=church,cathedral,chapel,bell_tower,mosque,minaret nwr/man_made=minaret \
    -o "$out/worship.osm.pbf" --overwrite
osmium export "$out/worship.osm.pbf" -f geojsonseq --geometry-types=point,polygon --overwrite -o - \
    | python3 "$(dirname "$0")/worship.py" "$out/worship.txt"
test "$(wc -l < "$out/worship.txt")" -gt 500000
