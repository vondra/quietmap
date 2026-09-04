#!/usr/bin/env bash
# Apply chronological IMD overlays to an immutable WorldCover base.
set -euo pipefail
cd "$(dirname "$0")/../.."
: "${IMD_SRC_ROOT:?set IMD_SRC_ROOT to the IMD source directory with year subdirectories}"
: "${IMD_BASE:?set IMD_BASE to the immutable WorldCover raw tile directory}"
: "${IMD_DST:?set IMD_DST to a separate finished IMD directory}"
exec "${QM_VENV_PYTHON:-$PWD/.venv/bin/python}" scripts/rasters/imd_overlay.py \
    --sources "$IMD_SRC_ROOT" --base "$IMD_BASE" --output "$IMD_DST" "$@"
