#!/usr/bin/env bash
# Point engine/<crate>/target at the workspace target directory.
#
# A workspace writes artifacts to engine/target/. Callers that still look
# under engine/<crate>/target/release then keep working. Re-runnable: a
# leftover per-crate cargo target is removed only when it looks like one
# (CACHEDIR.TAG or debug/release), never a random file.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd -P)
WS_TARGET="$ROOT/engine/target"
mkdir -p "$WS_TARGET"
crates=(
  aircraft-extract
  arrow-batching
  noise-compute
  noise-gpu
  osm-extract
  raster-reader
  source-reader
  tile-painter
)
for crate in "${crates[@]}"; do
  [ -d "$ROOT/engine/$crate" ] || continue
  link="$ROOT/engine/$crate/target"
  if [ -L "$link" ]; then
    [ "$(readlink -- "$link")" = "../target" ] && continue
    rm -f -- "$link"
  elif [ -e "$link" ]; then
    if [ -d "$link" ] && { [ -f "$link/CACHEDIR.TAG" ] || [ -d "$link/release" ] || [ -d "$link/debug" ]; }; then
      echo "ensure-engine-target-shims: replacing engine/$crate/target with a symlink to engine/target"
      rm -rf -- "$link"
    else
      echo "ensure-engine-target-shims: refusing unexpected path $link" >&2
      exit 1
    fi
  fi
  ln -s ../target "$link"
done
