#!/usr/bin/env python3
# Overture buildings parquet -> per-H3R4-cell obstacles.arrow (STAGING).
# Geodata-v2 step 1.1: the vector obstacle store for screening. Height ladder
# (finite height > 0 -> num_floors x 3 m -> 8 m) mirrors
# noise_compute::constants::BUILDING_{FLOOR_HEIGHT,DEFAULT_HEIGHT}_M (third
# mirror alongside the shell rasterizer — resync on change).
#
# On-disk contract (the Wave-1 promotion/reader depends on all three):
#   - polygon_wkb is 2D XY WKB, Polygon or MultiPolygon, passed through
#     unmodified from Overture; a reader must enumerate MultiPolygon parts.
#   - A footprint belongs to EXACTLY ONE (cell, tile) shard: its centroid's
#     H3 R4 cell, and the 1-degree tile that OWNS the centroid half-open
#     ([lat, lat+1) x [lon, lon+1)) — Overture bbox downloads overlap at tile
#     borders (200 shared footprints measured between N49E014/N50E014), so
#     rows whose centroid falls outside the named tile are skipped as
#     foreign; the neighbouring tile stages them.
#   - Because assignment is centroid-based, any consumer building a
#     tile+halo index MUST read the R4 cells of the halo plus at least
#     grid_disk(1) — a border-straddling footprint lives in its centroid's
#     cell only.
#
# Reads:  data/enrichment/global/overture-buildings/parquet/<TILE>.parquet
# Writes: data/enrichment/global/overture-obstacles/h3r4/<cell>/obstacles-<TILE>.arrow
#         (staging tree; promotion into data/prepared/{year}/h3r4 happens at
#          the Wave-1 cutover, never here). Re-running a tile first removes
#          that tile's existing shards so moved/vanished rows can't go stale.
#
# Usage: ingest-overture-obstacles.py N50E014 [N49E014 ...]

import glob
import math
import os
import sys

import h3
import pyarrow as pa
import pyarrow.ipc as ipc
import pyarrow.parquet as pq
from osgeo import ogr

FLOOR_HEIGHT = 3.0    # = BUILDING_FLOOR_HEIGHT_M
DEFAULT_HEIGHT = 8.0  # = BUILDING_DEFAULT_HEIGHT_M

PARQUET_DIR = "data/enrichment/global/overture-buildings/parquet"
OUT_DIR = "data/enrichment/global/overture-obstacles/h3r4"

SCHEMA = pa.schema(
    [
        ("polygon_wkb", pa.binary()),
        ("height_m", pa.float32()),
        ("centroid_lat", pa.float64()),
        ("centroid_lon", pa.float64()),
        # 0 = Overture row with mapped height, 1 = floors-derived, 2 = default.
        # Tiers 3 (city-measured zonal) and 4 (GHS-BUILT-H ANBH areal prior)
        # are written AFTER promotion by enrich-obstacle-heights.py — this
        # ingest only ever emits 0/1/2 (full ladder: that script's header and
        # noise_compute::low_profile).
        ("height_tier", pa.uint8()),
    ]
)

POLYGON_TYPES = {ogr.wkbPolygon, ogr.wkbMultiPolygon}


def ladder(h, f):
    # isfinite: +inf would pass `> 0`, survive the f32 cast, and then be
    # silently DROPPED by the Rust index's is_finite gate — a vanished
    # footprint instead of a floors/default fallback (gg review 2026-07-28).
    if h is not None and math.isfinite(h) and h > 0:
        return float(h), 0
    if f is not None and math.isfinite(f) and f > 0:
        return float(f) * FLOOR_HEIGHT, 1
    return DEFAULT_HEIGHT, 2


def tile_bounds(name: str):
    lat = int(name[1:3])
    lon = int(name[4:7])
    if name[0] == "S":
        lat = -lat
    if name[3] == "W":
        lon = -lon
    return lat, lon


def process_tile(name: str) -> None:
    src = os.path.join(PARQUET_DIR, f"{name}.parquet")
    lat0, lon0 = tile_bounds(name)
    schema_names = pq.read_schema(src).names
    cols = ["geometry", "height"]
    has_floors = "num_floors" in schema_names
    if has_floors:
        cols.append("num_floors")
    t = pq.read_table(src, columns=cols)
    geoms = t.column("geometry").to_pylist()
    heights = t.column("height").to_pylist()
    floors = t.column("num_floors").to_pylist() if has_floors else [None] * len(t)

    per_cell = {}
    skipped = 0
    foreign = 0
    for g, h, f in zip(geoms, heights, floors):
        if g is None:
            skipped += 1
            continue
        geom = ogr.CreateGeometryFromWkb(bytes(g))
        if (
            geom is None
            or geom.IsEmpty()
            or ogr.GT_Flatten(geom.GetGeometryType()) not in POLYGON_TYPES
        ):
            skipped += 1
            continue
        c = geom.Centroid()
        lon, lat = c.GetX(), c.GetY()
        if not (math.isfinite(lat) and math.isfinite(lon)):
            skipped += 1
            continue
        # Half-open centroid ownership: border footprints appear in BOTH
        # tiles' bbox downloads; exactly one tile stages them.
        if not (lat0 <= lat < lat0 + 1 and lon0 <= lon < lon0 + 1):
            foreign += 1
            continue
        height, tier = ladder(h, f)
        cell = h3.latlng_to_cell(lat, lon, 4)
        per_cell.setdefault(cell, []).append((bytes(g), height, lat, lon, tier))

    # Reconcile before writing: a re-ingest after an upstream re-extract may
    # move rows between cells — this tile's old shards must not survive it.
    for stale in glob.glob(os.path.join(OUT_DIR, "*", f"obstacles-{name}.arrow")):
        os.unlink(stale)

    for cell, rows in per_cell.items():
        d = os.path.join(OUT_DIR, cell)
        os.makedirs(d, exist_ok=True)
        # One arrow file per (cell, source-tile): world ingest runs per
        # 1-degree tile, and a cell can straddle tiles - merging happens at
        # promotion.
        out = os.path.join(d, f"obstacles-{name}.arrow")
        tbl = pa.table(
            {
                "polygon_wkb": pa.array([r[0] for r in rows], pa.binary()),
                "height_m": pa.array([r[1] for r in rows], pa.float32()),
                "centroid_lat": pa.array([r[2] for r in rows], pa.float64()),
                "centroid_lon": pa.array([r[3] for r in rows], pa.float64()),
                "height_tier": pa.array([r[4] for r in rows], pa.uint8()),
            },
            schema=SCHEMA,
        )
        tmp = out + ".tmp"
        with ipc.new_file(tmp, SCHEMA) as w:
            w.write_table(tbl)
        os.replace(tmp, out)

    total = sum(len(r) for r in per_cell.values())
    print(
        f"{name}: {total} obstacles -> {len(per_cell)} cells "
        f"(skipped {skipped}, foreign {foreign})"
    )


if __name__ == "__main__":
    for tile in sys.argv[1:]:
        process_tile(tile)
