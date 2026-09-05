#!/usr/bin/env python3
"""Derive coverage from existing official source catalogs and run the one native z9 publisher."""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess

import dem_sources
import worldcover_sources


def source_coverage(channel: str, dem_source: Path, worldcover_source: Path | None) -> dict:
    land = dem_sources.read_catalog(dem_source)[90]
    authority: dict[str, object] = {"glo90_land": sorted(land)}
    unknown = set()
    if channel == "imd":
        if worldcover_source is None:
            raise ValueError("IMD requires the official WorldCover source catalog")
        inventory = worldcover_sources.read_catalog(worldcover_source)
        tiles = set().union(*(worldcover_sources.native_tiles(key) for key in inventory))
        # ESA WorldCover PUM V2.0 §3.4.2 limits valid coverage to 82.75°N.
        # At the native input's one-degree granularity, partial N82 land is
        # deliberately unavailable until a source-backed per-node mask exists.
        unknown = (land - tiles) | {tile for tile in land if tile[0] >= 82}
        tiles -= unknown
        authority["worldcover"] = sorted(inventory.items())
    else:
        tiles = land
    return {"channel": channel, "tiles": sorted(tiles), "unknown": sorted(unknown),
            "authority": hashlib.sha256(json.dumps(authority, sort_keys=True).encode()).hexdigest()}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("plan", "publish"))
    parser.add_argument("--channel", choices=("dem", "forest", "imd"), required=True)
    parser.add_argument("--source-dir", type=Path, required=True, help="Verified, frozen 3601-square native HGT/raw tree")
    parser.add_argument("--dem-source", type=Path, required=True, help="Official GLO30/GLO90 catalog root")
    parser.add_argument("--worldcover-source", type=Path)
    parser.add_argument("--output", type=Path, required=True, help="Prepared root containing z9 and rasters.sqlite")
    args = parser.parse_args()
    coverage = source_coverage(args.channel, args.dem_source, args.worldcover_source)
    binary = Path(__file__).resolve().parents[2] / "engine/target/release/raster-repack"
    result = subprocess.run([str(binary), args.mode, str(args.source_dir), str(args.output)],
                            input=json.dumps(coverage, sort_keys=True), text=True, check=False)
    return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
