#!/usr/bin/env python3
"""The whole-world land tile census for the Overture building download.

The Planet-extracted H3 R4 inventory is the source of truth: every prepared
cell gets a structures.arrow (empty where nothing stands), so the download list
must cover every prepared cell — deriving it from a footprint-bearing subset
would permanently exclude empty land. Each H3 R4 cell contributes every
1-degree tile its bounding box touches.

The dataset year comes from the product contract, so a Planet re-extract stays
aligned without an edited latitude bound.
"""
import json
import math
import os
import sys
from pathlib import Path

def tile_name(lat_floor: int, lon_floor: int) -> str:
    ns = "S" if lat_floor < 0 else "N"
    ew = "W" if lon_floor < 0 else "E"
    return f"{ns}{abs(lat_floor):02d}{ew}{abs(lon_floor):03d}"


def default_source() -> Path:
    root = Path(__file__).resolve().parents[2]
    year = json.loads((root / "scripts/dataset-year.json").read_text())["current_year"]
    return Path(os.environ.get("QM_WORLD_TILE_SOURCE", root / "data/prepared" / year / "h3r4"))


def cell_degree_tiles(cell: str, h3_module) -> list[str]:
    """Every 1-degree tile the cell's boundary bounding box touches — the set
    whose parquets build-structures.py reads for this cell."""
    boundary = h3_module.cell_to_boundary(cell)
    lats = [p[0] for p in boundary]
    lons = [p[1] for p in boundary]
    # An antimeridian-straddling cell reports a ~360 degree span; unwrap it so the
    # range below walks the few tiles it really touches, not the whole globe.
    if max(lons) - min(lons) > 180:
        lons = [lon if lon >= 0 else lon + 360 for lon in lons]
    tiles = []
    for lat_floor in range(math.floor(min(lats)), math.floor(max(lats)) + 1):
        for lon_floor in range(math.floor(min(lons)), math.floor(max(lons)) + 1):
            tiles.append(tile_name(lat_floor, ((lon_floor + 180) % 360) - 180))
    return tiles


def census(source: str, h3_module=None) -> list[str]:
    if h3_module is None:
        import h3 as h3_module

    tiles: set[str] = set()
    for cell in os.listdir(source):
        if not h3_module.is_valid_cell(cell) or h3_module.get_resolution(cell) != 4:
            continue
        tiles.update(cell_degree_tiles(cell, h3_module))
    return sorted(tiles)


if __name__ == "__main__":
    source = sys.argv[1] if len(sys.argv) > 1 else default_source()
    if not os.path.isdir(source):
        sys.exit(f"prepared cell tree missing: {source}")
    print("\n".join(census(source)))
