#!/usr/bin/env python3
"""The world tile census: which 1-degree tiles the Overture building download covers.

This used to be `ls data/prepared/rasters/building/*.raw` — the vector ingest asked
the RASTER which tiles had ever seen a building. That made the worse representation
the source of truth for the better one, and it blocked deleting the raster at all.

The census now comes from the vector source itself (`overture-obstacles/h3r4/`), so
it maintains itself as new cells are ingested and needs no checked-in list. Each H3
R4 cell contributes every 1-degree tile its bounding box touches; the bbox, not the
boundary vertices, because a cell can cross a tile without putting a vertex in it
(measured: vertices alone missed S23W041).

Verified 2026-08-30 against the raster it replaces: 15 185 tiles derived, covering
all 13 694 raster tiles with zero gaps.
"""
import math
import os
import sys

SOURCE = os.environ.get(
    "QM_OBSTACLE_SOURCE",
    "data/enrichment/global/overture-obstacles/h3r4",
)


def tile_name(lat_floor: int, lon_floor: int) -> str:
    ns = "S" if lat_floor < 0 else "N"
    ew = "W" if lon_floor < 0 else "E"
    return f"{ns}{abs(lat_floor):02d}{ew}{abs(lon_floor):03d}"


def census(source: str) -> list[str]:
    import h3

    tiles: set[str] = set()
    for cell in os.listdir(source):
        if len(cell) != 15:
            continue
        boundary = h3.cell_to_boundary(cell)
        lats = [p[0] for p in boundary]
        lons = [p[1] for p in boundary]
        # An antimeridian-straddling cell reports a ~360 degree span; unwrap it so the
        # range below walks the few tiles it really touches, not the whole globe.
        if max(lons) - min(lons) > 180:
            lons = [lon if lon >= 0 else lon + 360 for lon in lons]
        for lat_floor in range(math.floor(min(lats)), math.floor(max(lats)) + 1):
            for lon_floor in range(math.floor(min(lons)), math.floor(max(lons)) + 1):
                tiles.add(tile_name(lat_floor, ((lon_floor + 180) % 360) - 180))
    return sorted(tiles)


if __name__ == "__main__":
    source = sys.argv[1] if len(sys.argv) > 1 else SOURCE
    if not os.path.isdir(source):
        sys.exit(f"obstacle source missing: {source}")
    print("\n".join(census(source)))
