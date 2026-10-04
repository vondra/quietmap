#!/usr/bin/env bash
# The bars, pubs, nightclubs, beer gardens, restaurants, cafes and fast-food places of an
# OpenStreetMap planet file (ODbL) for the people converter: the features alone (osmium
# tags-filter), then each as a point with its kind, terrace tag, area, opening hours and name
# (`qm-build dev4 --kinds sources --venues OUT_DIR/venues.txt`). Needs osmium-tool and Python 3.
#
#   venues.sh PLANET.osm.pbf OUT_DIR
set -euo pipefail
planet=$1 out=$2
mkdir -p "$out"
osmium tags-filter "$planet" \
    nwr/amenity=bar,pub,nightclub,biergarten,restaurant,cafe,fast_food,food_court \
    -o "$out/venues.osm.pbf" --overwrite
osmium export "$out/venues.osm.pbf" -f geojsonseq --geometry-types=point,polygon --overwrite -o - \
    | python3 "$(dirname "$0")/venues.py" "$out/venues.txt"
test "$(wc -l < "$out/venues.txt")" -gt 1000000
