#!/usr/bin/env bash
# The bus, trolleybus and coach routes of an OpenStreetMap planet file (ODbL) for the road
# converter: the route relations alone (osmium tags-filter), then per way the directions the
# routes serve and the departures of those that tag their interval
# (`qm-build dev4 --kinds sources --bus OUT_DIR/bus-ways.txt`). Needs osmium-tool and Python 3.
#
#   bus.sh PLANET.osm.pbf OUT_DIR
set -euo pipefail
planet=$1 out=$2
mkdir -p "$out"
osmium tags-filter "$planet" r/route=bus,trolleybus,coach -R -o "$out/bus-routes.osm.pbf" --overwrite
osmium cat "$out/bus-routes.osm.pbf" -f opl | python3 "$(dirname "$0")/bus-ways.py" "$out/bus-ways.txt"
test "$(wc -l < "$out/bus-ways.txt")" -gt 1000000
