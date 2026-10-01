#!/usr/bin/env bash
# Fetches the yearly mean air temperature for the road converter's CNOSSOS-EU temperature
# correction (2.2.2, Eq. 2.2.10): WorldClim 2.1 `bio_1` (1970-2000 means, 10 arc-minutes; Fick and
# Hijmans 2017, CC BY 4.0), written as raw little-endian f32, 2,160 x 1,080 cells from 90 N and
# 180 W, the sea -3.4e38 (`qm-build dev4 --kinds sources --temperature OUT_DIR/bio1.f32`).
#
#   worldclim.sh OUT_DIR
set -euo pipefail
out=$1
mkdir -p "$out"
curl -sS -L -o "$out/wc2.1_10m_bio.zip" \
    https://geodata.ucdavis.edu/climate/worldclim/2_1/base/wc2.1_10m_bio.zip
unzip -o -q "$out/wc2.1_10m_bio.zip" wc2.1_10m_bio_1.tif -d "$out"
gdal_translate -q -of ENVI -ot Float32 "$out/wc2.1_10m_bio_1.tif" "$out/bio1.f32"
test "$(stat -c %s "$out/bio1.f32")" -eq $((2160 * 1080 * 4))
