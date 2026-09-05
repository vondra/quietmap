#!/usr/bin/env bash
# Fetch public GLO30 and only catalog-derived GLO90 gaps plus native kernel support.
set -euo pipefail
cd "$(dirname "$0")/../.."
: "${DEM_SRC:?set DEM_SRC to the complete Copernicus source root}"
JOBS="${JOBS:-8}"
python3 scripts/rasters/dem_sources.py catalog "$DEM_SRC"
aws s3 sync s3://copernicus-dem-30m/ "$DEM_SRC/glo30/" \
    --no-sign-request --only-show-errors --exclude '*' --include '*_DEM.tif'
python3 scripts/rasters/dem_sources.py supplement "$DEM_SRC" "$JOBS"
