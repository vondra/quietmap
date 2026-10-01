#!/usr/bin/env bash
# Fetches the TEN-T rail network for the rail converter's freight allocation: the GISCO layer
# `railways_GL2017_EU` (the TEN-T maps of Regulation 1315/2013, EU27) as the European Union Agency
# for Railways republishes it, and keeps every line but the high-speed passenger lines (a line
# the maps call passenger may carry heavy freight: the left Rhine line, 76 freight trains a day at
# the EBA monitor) as one line per line string: `<tier> x0 y0 x1 y1 ...` in Web Mercator metres,
# tier 2 on a core network corridor, 1 elsewhere (`qm-build dev4 --kinds sources --tent
# OUT_DIR/tent-freight.txt`). Needs GDAL's ogr2ogr.
#
#   tent.sh OUT_DIR
set -euo pipefail
out=$1
mkdir -p "$out"
base=https://api.triplydb.com/datasets/era/gisco/assets
for part in shp:6290b36e4473e2a7e8d45851/6290b36e4473e2a7e8d45852 \
    dbf:6290b36d957b26400bad4b64/6290b36d957b26400bad4b65 \
    shx:6290b36d957b26400bad4b3d/6290b36d957b26400bad4b3e \
    prj:6290b36d957b26400bad4b47/6290b36d957b26400bad4b48; do
    curl -sS -L -o "$out/railways_GL2017_EU.${part%%:*}" "$base/${part#*:}"
done
ogr2ogr -f CSV /vsistdout/ "$out/railways_GL2017_EU.shp" -lco GEOMETRY=AS_WKT \
    -select CORRIDORS,RAILWAYS_A,TYPE |
    awk -F'"' 'NR > 1 && $3 !~ /,Passenger,High speed$/ {
        tier = ($3 ~ /^,[A-Z]+,/) ? 2 : 1
        line = $2; sub(/^LINESTRING \(/, "", line); sub(/\)$/, "", line)
        gsub(/,/, " ", line)
        print tier, line
    }' > "$out/tent-freight.txt"
test "$(wc -l < "$out/tent-freight.txt")" -gt 2000
