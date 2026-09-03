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
# Reads:  data/source/enrichment/global/overture-buildings/parquet/<TILE>.parquet
# Writes: data/source/enrichment/global/overture-obstacles/h3r4/<cell>/obstacles-<TILE>.arrow
#         (source staging tree). Re-running a tile first removes that tile's
#         existing shards so moved/vanished rows can't go stale.
#         scripts/obstacles/enrich-obstacle-heights.py merges these shards into
#         each prepared cell's obstacles.arrow, and writes an EMPTY one where the
#         finished sweep found nothing — emptiness is per cell, so no painter
#         reads anything world-wide.
#
# .ingested-tiles is this ingest's own resume bookkeeping (which 1-degree tiles
# are done). Its ONE other consumer is that promotion, which uses it to tell a
# swept-and-empty cell from an unswept one; nothing at paint time reads it.
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

PARQUET_DIR = "data/source/enrichment/global/overture-buildings/parquet"
OUT_DIR = "data/source/enrichment/global/overture-obstacles/h3r4"

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
        ("envelope_class", pa.uint8()),
    ]
)

POLYGON_TYPES = {ogr.wkbPolygon, ogr.wkbMultiPolygon}
OUTDOOR = {
    "carport",
    "roof",
    "greenhouse",
    "glasshouse",
    "bridge_structure",
    "grandstand",
}
RESIDENTIAL = {
    "allotment_house", "apartments", "beach_hut", "boathouse", "bungalow",
    "cabin", "college", "detached", "dormitory", "dwelling_house", "ger",
    "hospital", "house", "houseboat", "hut", "kindergarten", "residential",
    "school", "semi", "semidetached_house", "static_caravan", "stilt_house",
    "terrace", "trullo", "university",
}
COMMERCIAL = {"commercial", "hotel", "office", "retail", "supermarket"}
INDUSTRIAL = {
    "agricultural", "barn", "cowshed", "digester", "factory", "farm",
    "farm_auxiliary", "hangar", "industrial", "manufacture", "shed", "silo",
    "slurry_tank", "stable", "storage_tank", "sty", "warehouse",
}
HISTORIC = {
    "cathedral", "chapel", "church", "civic", "fire_station", "government",
    "library", "monastery", "mosque", "post_office", "presbytery", "public",
    "religious", "shrine", "synagogue", "temple", "wayside_shrine",
}
# Published official BuildingClass remainder. Named here so known remainder
# classes stay 5 instead of taking the subtype fallback (`garage` +
# `residential` would otherwise become 1).
DEFAULT = {
    "garage", "garages", "kiosk", "service", "parking", "stadium",
    "sports_centre", "sports_hall", "pavilion", "toilets", "bunker", "military",
    "transportation", "train_station", "transformer_tower", "outbuilding",
    "guardhouse",
}
OFFICIAL_CLASSES = OUTDOOR | RESIDENTIAL | COMMERCIAL | INDUSTRIAL | HISTORIC | DEFAULT
SUBTYPE = {
    "residential": 1,
    "education": 1,
    "medical": 1,
    "commercial": 2,
    "agricultural": 3,
    "industrial": 3,
    "civic": 4,
    "religious": 4,
}


def envelope_class(building_class, subtype, underground):
    if underground:
        return 0
    if building_class in OUTDOOR:
        return 0
    if building_class in RESIDENTIAL:
        return 1
    if building_class in COMMERCIAL:
        return 2
    if building_class in INDUSTRIAL:
        return 3
    if building_class in HISTORIC:
        return 4
    # A null or unknown class is not evidence for DEFAULT: class refines
    # subtype, so only that case may use the official subtype fallback.
    if building_class is None or building_class not in OFFICIAL_CLASSES:
        return SUBTYPE.get(subtype, 5)
    return 5


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
    for column in ("class", "subtype", "is_underground"):
        if column in schema_names: cols.append(column)
    t = pq.read_table(src, columns=cols)
    geoms = t.column("geometry").to_pylist()
    heights = t.column("height").to_pylist()
    floors = t.column("num_floors").to_pylist() if has_floors else [None] * len(t)
    classes = t.column("class").to_pylist() if "class" in schema_names else [None] * len(t)
    subtypes = t.column("subtype").to_pylist() if "subtype" in schema_names else [None] * len(t)
    underground = t.column("is_underground").to_pylist() if "is_underground" in schema_names else [False] * len(t)

    per_cell = {}
    skipped = 0
    foreign = 0
    for g, h, f, bc, st, ug in zip(geoms, heights, floors, classes, subtypes, underground):
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
        per_cell.setdefault(cell, []).append((bytes(g), height, lat, lon, tier, envelope_class(bc, st, bool(ug))))

    # Reconcile before writing: a re-ingest after an upstream re-extract may
    # move rows between cells — this tile's old shards must not survive it.
    # The resume entry goes FIRST: while this tile's shards are being rewritten,
    # a shard-less cell must read "unswept", so the promotion leaves it alone
    # instead of materializing it EMPTY from a half-written tile.
    # ingest-world-incremental.sh re-appends the tile on success.
    manifest = "data/source/enrichment/global/overture-obstacles/.ingested-tiles"
    if os.path.exists(manifest):
        with open(manifest) as f:
            lines = f.readlines()
        kept = [l for l in lines if l.strip() != name]
        if len(kept) != len(lines):
            with open(manifest, "w") as f:
                f.writelines(kept)
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
                "envelope_class": pa.array([r[5] for r in rows], pa.uint8()),
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
