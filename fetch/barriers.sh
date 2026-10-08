#!/usr/bin/env bash
# The noise screens of national topographic databases for the obstacles (`qm-build dev4 --kinds
# obstacles --barriers OUT_DIR/barriers.txt`), one per line: `source height_m lat lon lat lon ...`
# (tab-separated; height 0 when the database gives none). Poland's BDOT10k (GUGiK; "dostępna
# bezpłatnie i możliwa do dowolnego wykorzystania"): every county's package, found by trying each
# TERYT code, and of it the lines of class OT_OIKM_L of kind "ekran akustyczny" (a screen's axis).
# OpenStreetMap maps 839 of their 1,160 km beside Poland's motorways (evidence 2026-10-08, Poland).
# Needs curl, unzip and GDAL's ogr2ogr; the packages (about 17 GB) stay in OUT_DIR/bdot10k.
#
#   barriers.sh OUT_DIR
set -euo pipefail
out=$1
mkdir -p "$out/bdot10k"
base=https://opendata.geoportal.gov.pl/bdot10k/schemat2021/SHP
for province in 02 04 06 08 10 12 14 16 18 20 22 24 26 28 30 32; do
    for county in $(seq -w 1 80); do
        code=$province$county zip=$out/bdot10k/${code}_SHP.zip
        if [ ! -s "$zip" ] && ! curl -sS -f -L --retry 3 -o "$zip" "$base/$province/${code}_SHP.zip" 2> /dev/null; then
            rm -f "$zip"
            continue
        fi
        rm -rf "$out/bdot10k/$code" && mkdir "$out/bdot10k/$code"
        # A county without the class has no screens.
        unzip -j -o -q "$zip" '*__OT_OIKM_L.*' -d "$out/bdot10k/$code" || continue
        ogr2ogr -f CSV /vsistdout/ "$out/bdot10k/$code"/*__OT_OIKM_L.shp -t_srs EPSG:4326 \
            -where "RODZAJ = 'ekran akustyczny'" -select RODZAJ -lco GEOMETRY=AS_WKT -lco SEPARATOR=TAB
    done
done | awk -F'\t' '$1 ~ /LINESTRING/ {
    # One line per part: WKT gives longitude first.
    wkt = $1; gsub(/"|MULTILINESTRING|LINESTRING|\(\(|\)\)/, "", wkt); gsub(/^ *\(|\) *$/, "", wkt)
    n = split(wkt, parts, /\), ?\(/)
    for (k = 1; k <= n; k++) {
        m = split(parts[k], points, ","); line = "bdot10k\t0"
        for (i = 1; i <= m; i++) { split(points[i], xy, " "); line = line "\t" xy[2] "\t" xy[1] }
        if (m >= 2) print line
    }
}' > "$out/barriers.txt"
test "$(wc -l < "$out/barriers.txt")" -gt 5000
