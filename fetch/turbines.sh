#!/usr/bin/env bash
# The wind turbines standing today for the sources (`qm-build dev4 --kinds sources --turbines
# OUT_DIR/turbines.txt`): OpenStreetMap's (ODbL; osmium tags-filter of a planet file), and the
# national registers with turbine positions and states, each turbine once (`turbines.py`):
# USWTDB (US, public domain), NRCan (Canada, OGL-Canada), the Marktstammdatenregister (Germany,
# dl-de/by-2-0, "Marktstammdatenregister, Bundesnetzagentur"), Energistyrelsen (Denmark),
# Vindbrukskollen (Sweden, CC0), NVE (Norway, NLOD 2.0), Castilla-La Mancha (Spain, CC BY-SA) and
# RIVM (the Netherlands and Belgium, public domain). Needs curl, unzip, osmium-tool and Python 3
# with GDAL's bindings (osgeo).
#
#   turbines.sh PLANET.osm.pbf OUT_DIR
set -euo pipefail
planet=$1 out=$2
registers=$out/registers
mkdir -p "$registers"/{uswtdb,nrcan,mastr,ens,vindbrukskollen,nve,clm,rivm}
get() { curl -sS -L --fail --retry 3 --max-time 3600 -A Mozilla/5.0 -o "$2" "$1"; }
get https://energy.usgs.gov/uswtdb/assets/data/uswtdbCSV.zip "$registers/uswtdb/uswtdb.zip"
unzip -o -q "$registers/uswtdb/uswtdb.zip" -d "$registers/uswtdb"
get https://ftp.cartes.canada.ca/pub/nrcan_rncan/Wind-energy_Energie-eolienne/wind_turbines_database/Wind_Turbine_Database_en.xlsx \
    "$registers/nrcan/Wind_Turbine_Database_en.xlsx"
# The Marktstammdatenregister's export changes its name daily: the link of the download page.
mastr=$(curl -sS -L -A Mozilla/5.0 https://www.marktstammdatenregister.de/MaStR/Datendownload |
    grep -o 'https://download.marktstammdatenregister.de/Gesamtdatenexport_[0-9]*_[0-9.]*\.zip' | head -1)
get "$mastr" "$registers/mastr/export.zip"
unzip -o -q "$registers/mastr/export.zip" 'EinheitenWind*.xml' -d "$registers/mastr"
rm "$registers/mastr/export.zip"
get https://ens.dk/media/8748/download "$registers/ens/current.xlsx"
get https://ens.dk/media/8746/download "$registers/ens/historic.xlsx"
get https://ext-dokument.lansstyrelsen.se/gemensamt/geodata/ShapeExport/lst.vbk_vindkraftverk.zip \
    "$registers/vindbrukskollen/vindbrukskollen.zip"
unzip -o -q "$registers/vindbrukskollen/vindbrukskollen.zip" -d "$registers/vindbrukskollen"
nve=https://kart.nve.no/enterprise/rest/services/Vindkraft2/MapServer
get "$nve/0/query?where=1%3D1&outFields=*&outSR=4326&f=geojson" "$registers/nve/parks.geojson"
get "$nve/4/query?where=1%3D1&outFields=*&outSR=4326&f=geojson" "$registers/nve/turbines.geojson"
get 'https://datosabiertos.castillalamancha.es/sites/datosabiertos.castillalamancha.es/files/AEROGENERADORES%20CLM.csv' \
    "$registers/clm/turbines.csv"
get 'https://data.rivm.nl/geo/alo/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=alo:rivm_windturbines_ashoogte_actueel&outputFormat=application/json' \
    "$registers/rivm/turbines.geojson"
osmium tags-filter "$planet" nwr/generator:source=wind nwr/man_made=wind_turbine \
    -o "$out/turbines.osm.pbf" --overwrite
osmium export "$out/turbines.osm.pbf" -f geojsonseq -a type,id --overwrite -o - \
    | python3 "$(dirname "$0")/turbines.py" "$registers" "$out/turbines.txt"
test "$(wc -l < "$out/turbines.txt")" -gt 450000
