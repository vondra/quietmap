#!/usr/bin/env bash
# Download GHSL Built-H R2023A (global building height, 100m).
# ~1.88 GB, ~5 min. No auth (JRC open data).
#
# Required env: GHSL_DST (the release buildings source dir).
set -euo pipefail

: "${GHSL_DST:?set GHSL_DST to the release buildings source dir}"
# NOTE: exact zip URL UNVERIFIED (JRC tree walk 2026-09-04 ended at an empty
# version dir; the frozen R2023A copy seeded the release instead). Re-verify
# the path before the next fresh download.
ZIP="$GHSL_DST/[REDACTED].zip"
URL="https://jeodpp.jrc.ec.europa.eu/ftp/jrc-opendata/GHSL/[REDACTED]/[REDACTED]/V1-0/[REDACTED].zip"

mkdir -p "$GHSL_DST"

if [ -f "$ZIP" ]; then
    echo "[ghsl] $(date '+%H:%M:%S') Already downloaded: $ZIP ($(du -sh "$ZIP" | cut -f1))"
else
    echo "[ghsl] $(date '+%H:%M:%S') Downloading GHSL Built-H R2023A ..."
    # -g: the filename contains literal [...] (curl would glob them).
    curl -g -L --retry 5 --retry-all-errors -o "$ZIP" "$URL"
fi

echo "[ghsl] $(date '+%H:%M:%S') Extracting ..."
unzip -o -q "$ZIP" -d "$GHSL_DST/"
COUNT=$(ls "$GHSL_DST"/*.tif 2>/dev/null | wc -l)
echo "[ghsl] $(date '+%H:%M:%S') Done: $COUNT GeoTIFF tiles"
