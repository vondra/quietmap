#!/usr/bin/env bash
# Download one dated OSM planet PBF, resumable.
#
#   scripts/source/download-planet.sh 260831
#   PBF_DIR=/data/.../source/2026/osm scripts/source/download-planet.sh 260902
#
# Writes $PBF_DIR/planet-YYMMDD.osm.pbf (curl -C - : rerun resumes).
# Pick the Monday dated file from https://planet.openstreetmap.org/pbf/ —
# the release name embeds the same date (source/2026/osm/planet-260831…).
set -euo pipefail

DATE="${1:?usage: $0 YYMMDD  (e.g. $0 260831)}"
PBF_DIR="${PBF_DIR:-/data/readmostly1/r260904/source/2026/osm}"
mkdir -p "$PBF_DIR"
exec curl -L -C - --retry 5 --retry-all-errors \
    -o "$PBF_DIR/planet-$DATE.osm.pbf" \
    "https://planet.openstreetmap.org/pbf/planet-$DATE.osm.pbf"
