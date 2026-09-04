#!/usr/bin/env python3
# build-structures.py — write the ONE per-cell structure table: structures.arrow.
#
# Every prepared H3 R4 cell holds structures.arrow — merged where anything stands,
# a 0-row table with the schema below where nothing does. One row per physical
# structure carries the union of:
#   * OSM buildings (buildings.arrow as osm-extract + the enrich chain emit it,
#     in the --osm-dir source tree) — the EMISSION stock: their attributes and
#     polygons drive the building layer;
#   * Overture footprints (the screening stock) — a matched OSM<->Overture pair
#     shares one row; an Overture footprint with no OSM twin is attribute-less;
#   * OSM noise walls (barriers.arrow) as kind=barrier polyline micro-segments.
#
# Semantics contract (engine readers rely on it):
#   * emission reads kind=0 rows with osm_id present, in file order — exactly
#     today's buildings.arrow subsequence with the same values, so the building
#     layer is unchanged; the emission polygon is emission_polygon_wkb ??
#     geometry_wkb, stored only where emission can read it (area missing or
#     > 2000 m2, the grid-split threshold in noise_compute::normalize::points)
#     and different from the screening polygon; the emission position is
#     emission_centroid_* ?? centroid_*;
#   * screening reads every row with geometry; a matched pair keeps the OVERTURE
#     polygon and the Overture-side ladder height (census 2026-09-03: 0 of 2.83 M
#     matched pairs share WKB, so OSM geometry on matched rows would repaint the
#     world; OSM height upgrades on matched rows would re-height 18,743 Dobris
#     buildings against the absolute z12 byte proof — both deliberately NOT
#     applied; OSM-only rows ladder from their own OSM tags and screen for the
#     first time, which is the GOAL's "screening reads every row");
#   * walls screen as ObstacleKind::Barrier polylines inside the same index;
#   * airborne reads kind=0 polygons and ignores barriers, exactly as today.
#
# Matching rule (census-justified: the overlap-IoU distribution is bimodal with a
# deep 0.1..0.9 valley, so any threshold in [0.3, 0.8] separates identically up to
# ±0.5 % of matches): an Overture footprint matches an OSM building iff the
# Overture centroid lies in the OSM polygon OR IoU >= 0.5. Assignment is
# one-to-one, greedy over the COMPLETE qualifying pair set by (iou desc,
# centroid_in desc, overture row asc, osm row asc) — deterministic, and a row
# whose first choice is taken falls through to its next qualifying twin instead
# of screening the same structure twice. Leftover Overture rows of a split
# building become Overture-only rows (screening then sees exactly today's
# Overture set); leftover OSM rows stay OSM-only.
#
# Height ladder (tier semantics are load-time contract in
# noise_compute::low_profile, which caps tiers 2/4 to 3 m next to small OSM
# buildings — kind=0 rows only):
#   tier 0 mapped height · 1 floors x 3 m · 2 flat 8 m default ·
#   3 regional measured zonal (replaces 1/2) · 4 GHSL ANBH prior (replaces 2 only)
# --validate proves the emission view against buildings.arrow row by row, so a
# merge that would move the building layer fails the cell instead of painting it.
#
# Overture source, two modes:
#   --overture-shards DIR   migration bridge: the per-cell staging shard tree
#                           (deleted once every cell has its table; this mode
#                           and its reader are deleted with it);
#   --overture-parquet DIR  final design: the downloaded one-degree parquets
#                           (download-overture-tiles.py; they are kept now).
#
# Per cell: deterministic, idempotent (rebuild iff an input is newer than the
# output — the cell's two arrows, the Overture source and both ladder rasters,
# so neither a new Overture release nor a refreshed raster tile is served
# stale), re-runnable in any order, atomic (tmp+fsync+rename, dir fsync), never
# creates a prepared cell directory (the table follows the prepared inventory)
# and never removes one. buildings.arrow and barriers.arrow are the OSM inputs
# every rebuild reads again; they live in the --osm-dir SOURCE tree, never in a
# prepared cell, which holds only what the painters read.
#
# Usage:
#   build-structures.py --h3r4-dir data/prepared/2026/h3r4 \
#     --osm-dir data/source/osm-extract/2026/h3r4 \
#     (--overture-shards DIR | --overture-parquet DIR) \
#     --ghsl <ANBH.tif> [--regional <mosaic.vrt>] \
#     (--cells hex,... | --cells-file F) [--validate]

import argparse
import glob
import json
import math
import os
import struct
import sys

import h3
import numpy as np
import pyarrow as pa
import pyarrow.compute as pc
import pyarrow.ipc as ipc
import pyarrow.parquet as pq
import shapely
import shapely.ops
from osgeo import gdal
from pyproj import Transformer
from shapely import STRtree
from shapely import wkb as shapely_wkb

gdal.UseExceptions()
gdal.SetCacheMax(512 * 1024 * 1024)  # ANBH point reads cluster; keep blocks hot

# ── Height ladder constants ──────────────────────────────────────────────────
MEASURED_MIN_M = 2.0      # zonal pixels below this are "not a building surface here"
COVERAGE_MIN_FRAC = 0.30  # measured pixels must cover this share of the footprint
COVERAGE_MIN_PX = 3
TIER3_CLAMP = (2.5, 250.0)
ANBH_MIN_M = 1.0          # ANBH below this = no better info than the default
ANBH_MAX_VALID = 250.0    # GHSL NoData sentinel is 255 — belt for a missing tag
TIER4_CLAMP = (3.0, 100.0)

FLOOR_HEIGHT = 3.0        # == noise_compute::constants::BUILDING_FLOOR_HEIGHT_M
DEFAULT_HEIGHT = 8.0      # == noise_compute::constants::BUILDING_DEFAULT_HEIGHT_M
WALL_DEFAULT_HEIGHT_M = 3.0  # osm-extract's barrier spill default (spill.rs)
EMISSION_GRID_THRESHOLD_M2 = 2000.0  # BUILDING_AREA_THRESHOLD_M2 (normalize/points.rs)

KIND_BUILDING = 0   # == ObstacleKind::Building.code()
KIND_BARRIER = 1    # == ObstacleKind::Barrier.code()

ENVELOPE_OUTDOOR = 0
ENVELOPE_RESIDENTIAL = 1
ENVELOPE_COMMERCIAL = 2
ENVELOPE_INDUSTRIAL = 3
ENVELOPE_DEFAULT = 5
# OSM building_use (osm-extract spill.rs): 0 residential, 1 commercial, 2 industrial.
# 0 is the extract's RESIDENTIAL code, not a missing value: write_buildings.rs
# declares the column non-nullable, so an untagged building arrives as 0 and the
# residential envelope is the extract's own answer (measured 2026-09-04: 0 nulls
# and 118,140 of 118,141 Dobris rows at 0).
ENVELOPE_FROM_BUILDING_USE = {
    0: ENVELOPE_RESIDENTIAL,
    1: ENVELOPE_COMMERCIAL,
    2: ENVELOPE_INDUSTRIAL,
}

IOU_MATCH_THRESHOLD = 0.5  # census: the overlap-IoU distribution is bimodal (>=0.9)

CONTRACT_KEY = "structures_contract"
CONTRACT_VERSION = "structures_v1"

SCHEMA = pa.schema(
    [
        pa.field("kind", pa.uint8(), nullable=False),
        pa.field("geometry_wkb", pa.binary()),
        pa.field("height_m", pa.float32(), nullable=False),
        pa.field("height_tier", pa.uint8(), nullable=False),
        pa.field("envelope_class", pa.uint8(), nullable=False),
        pa.field("centroid_lat", pa.float64(), nullable=False),
        pa.field("centroid_lon", pa.float64(), nullable=False),
        # OSM emission attributes — set exactly on OSM-attributed rows.
        pa.field("osm_id", pa.int64()),
        pa.field("building_type", pa.uint8()),
        pa.field("building_use", pa.uint8()),
        pa.field("height", pa.float32()),  # raw OSM height tag (emission input)
        pa.field("floors", pa.uint8()),
        pa.field("name", pa.utf8()),
        pa.field("addr_street", pa.utf8()),
        pa.field("addr_housenumber", pa.utf8()),
        pa.field("area_m2", pa.float32()),
        pa.field("opening_hours_frac", pa.uint8()),
        pa.field("source_id", pa.uint16()),
        # Emission overrides (null -> geometry_wkb / centroid_*): the OSM
        # polygon where emission can read it and it differs from the screening
        # polygon; the OSM centroid on matched rows (whose centroid_* is the
        # Overture one, which the low-profile cap reads as today).
        pa.field("emission_polygon_wkb", pa.binary()),
        pa.field("emission_centroid_lat", pa.float64()),
        pa.field("emission_centroid_lon", pa.float64()),
        # Wall micro-segment index (barrier rows only; ScreeningSourceId data).
        pa.field("segment_idx", pa.int16()),
        # Obstacle-index insertion order: the index's dense edge ids follow the
        # file's physical row order unless the loader sorts by this column —
        # the engine's crossing races resolve exact δ ties by scan order, so the
        # order the painted world was produced in has to survive every rebuild
        # (measured: order drives 89 of 90 differing tiles). Builders assign it;
        # loaders sort by it; null = never indexed (geometry-less
        # emission-only rows).
        pa.field("screening_ordinal", pa.uint32()),
    ]
)

BUILDINGS_COLUMNS = [
    "osm_id", "centroid_lat", "centroid_lon", "building_type", "building_use",
    "height", "floors", "name", "addr_street", "addr_housenumber", "polygon_wkb",
    "area_m2", "opening_hours_frac", "source_id",
]

# The columns the emission view is validated against, in buildings.arrow order.
EMISSION_COMPARE = [
    "osm_id", "building_type", "building_use", "height", "floors", "name",
    "addr_street", "addr_housenumber", "area_m2", "opening_hours_frac",
    "source_id",
]


# ── Raster height sources ────────────────────────────────────────────────────

def transformer_for(ds, path):
    """WGS84 -> the raster's own CRS. Hard-fails on an unreferenced raster —
    the IPR exportImage tiles arrive as a datum-less LOCAL_CS shell until the
    downloader stamps EPSG:5514 (gg review 2026-08-09, Codex CRITICAL 3)."""
    from pyproj import CRS

    crs = CRS.from_wkt(ds.GetProjection())
    if not (crs.is_projected or crs.is_geographic):
        raise SystemExit(f"{path}: raster CRS is not georeferenced ({crs.name}) — re-run scripts/obstacles/download-height-rasters.sh to stamp it")
    return Transformer.from_crs("EPSG:4326", crs, always_xy=True)


def raster_mtime(ds):
    """Newest mtime over every file the raster is made of — the height ladder is
    an input to every cell, so a refreshed tile behind a VRT mosaic has to
    invalidate the built tables just as the .vrt itself does."""
    return max(os.path.getmtime(f) for f in ds.GetFileList())


class GlobalPrior:
    """GHS-BUILT-H ANBH: nearest-pixel value at a WGS84 point (windowed reads)."""

    def __init__(self, path):
        self.ds = gdal.Open(path)
        self.band = self.ds.GetRasterBand(1)
        self.gt = self.ds.GetGeoTransform()
        self.nodata = self.band.GetNoDataValue()
        self.w, self.h = self.ds.RasterXSize, self.ds.RasterYSize
        self.tr = transformer_for(self.ds, path)
        self.mtime = raster_mtime(self.ds)

    def sample(self, lon, lat):
        x, y = self.tr.transform(lon, lat)
        ci = int((x - self.gt[0]) / self.gt[1])
        ri = int((y - self.gt[3]) / self.gt[5])
        if not (0 <= ci < self.w and 0 <= ri < self.h):
            return None
        v = float(self.band.ReadAsArray(ci, ri, 1, 1)[0, 0])
        if not math.isfinite(v) or v >= ANBH_MAX_VALID:
            return None
        if self.nodata is not None and v == self.nodata:
            return None
        return v


class RegionalHeights:
    """Regional 1 m relative-height raster held fully in RAM (a Praha-sized
    mosaic is 37 501 x 28 000 F32 ~ 4.2 GB; zonal over VRT windows would pay
    tile decompression per footprint)."""

    def __init__(self, path):
        ds = gdal.Open(path)
        self.gt = ds.GetGeoTransform()
        self.w, self.h = ds.RasterXSize, ds.RasterYSize
        self.tr = transformer_for(ds, path)
        self.mtime = raster_mtime(ds)
        band = ds.GetRasterBand(1)
        self.arr = band.ReadAsArray().astype(np.float32, copy=False)
        nodata = band.GetNoDataValue()
        if nodata is not None:
            self.arr[self.arr == nodata] = np.nan

    def covers(self, x, y):
        c = (x - self.gt[0]) / self.gt[1]
        r = (y - self.gt[3]) / self.gt[5]
        return 0 <= c < self.w and 0 <= r < self.h

    def zonal_measured_mean(self, geom_wgs84):
        """Mean of in-footprint pixels >= MEASURED_MIN_M, or None when the
        coverage guard says the city model does not know this structure."""
        g = shapely.ops.transform(self.tr.transform, geom_wgs84)
        minx, miny, maxx, maxy = g.bounds
        c0 = max(0, int(math.floor((minx - self.gt[0]) / self.gt[1])))
        c1 = min(self.w, int(math.ceil((maxx - self.gt[0]) / self.gt[1])) + 1)
        r0 = max(0, int(math.floor((maxy - self.gt[3]) / self.gt[5])))
        r1 = min(self.h, int(math.ceil((miny - self.gt[3]) / self.gt[5])) + 1)
        if c1 <= c0 or r1 <= r0:
            return None
        # A malformed continent-scale footprint would mesh-grid gigabytes here
        # (gg pass 2) — no real building needs a 4x4 km window; abstain.
        if (c1 - c0) * (r1 - r0) > 16_000_000:
            return None
        window = self.arr[r0:r1, c0:c1]
        xs = self.gt[0] + (np.arange(c0, c1) + 0.5) * self.gt[1]
        ys = self.gt[3] + (np.arange(r0, r1) + 0.5) * self.gt[5]
        xx, yy = np.meshgrid(xs, ys)
        inside = shapely.contains_xy(g, xx.ravel(), yy.ravel()).reshape(window.shape)
        vals = window[inside]
        vals = vals[np.isfinite(vals)]
        measured = vals[vals >= MEASURED_MIN_M]
        if len(measured) < max(COVERAGE_MIN_PX, COVERAGE_MIN_FRAC * int(inside.sum())):
            return None
        return float(measured.mean())


# ── Overture row sources ──────────────────────────────────────────────────────

def read_overture_shards(cell_dir):
    """Merge the cell's staging shards in sorted-filename order — the order the
    old loader read them, so the rebuilt stock reproduces the promoted row order.
    `None` means the cell has no shards at all.

    Returns (rows, newest_shard_mtime). A shard staged before envelope_class
    existed merges as the enclosed DEFAULT (one cell can hold shards from two
    ingest eras — measured 2026-09-03)."""
    shards = sorted(glob.glob(os.path.join(cell_dir, "obstacles-*.arrow")))
    if not shards:
        return None, None
    rows = []
    for shard in shards:
        table = ipc.open_file(shard).read_all()
        n = table.num_rows
        if "envelope_class" in table.column_names:
            env = table.column("envelope_class").to_pylist()
        else:
            env = [ENVELOPE_DEFAULT] * n
        for wkb, h, tier, clat, clon, e in zip(
            table.column("polygon_wkb").to_pylist(),
            table.column("height_m").to_pylist(),
            table.column("height_tier").to_pylist(),
            table.column("centroid_lat").to_pylist(),
            table.column("centroid_lon").to_pylist(),
            env,
        ):
            rows.append(
                {"wkb": wkb, "height_m": h, "tier": tier,
                 "clat": clat, "clon": clon, "envelope": e}
            )
    return rows, max(os.path.getmtime(s) for s in shards)


# Overture class/subtype -> envelope_class: the builder owns the whole
# ingest+ladder+merge, so the mapping lives here, once.
OUTDOOR_CLASSES = {
    "carport", "roof", "greenhouse", "glasshouse", "bridge_structure", "grandstand",
}
RESIDENTIAL_CLASSES = {
    "allotment_house", "apartments", "beach_hut", "boathouse", "bungalow",
    "cabin", "college", "detached", "dormitory", "dwelling_house", "ger",
    "hospital", "house", "houseboat", "hut", "kindergarten", "residential",
    "school", "semi", "semidetached_house", "static_caravan", "stilt_house",
    "terrace", "trullo", "university",
}
COMMERCIAL_CLASSES = {"commercial", "hotel", "office", "retail", "supermarket"}
INDUSTRIAL_CLASSES = {
    "agricultural", "barn", "cowshed", "digester", "factory", "farm",
    "farm_auxiliary", "hangar", "industrial", "manufacture", "shed", "silo",
    "slurry_tank", "stable", "storage_tank", "sty", "warehouse",
}
HISTORIC_CLASSES = {
    "cathedral", "chapel", "church", "civic", "fire_station", "government",
    "library", "monastery", "mosque", "post_office", "presbytery", "public",
    "religious", "shrine", "synagogue", "temple", "wayside_shrine",
}
# Published official BuildingClass remainder. Named so known remainder classes
# stay 5 instead of taking the subtype fallback (`garage` + `residential`
# would otherwise become 1).
DEFAULT_CLASSES = {
    "garage", "garages", "kiosk", "service", "parking", "stadium",
    "sports_centre", "sports_hall", "pavilion", "toilets", "bunker", "military",
    "transportation", "train_station", "transformer_tower", "outbuilding",
    "guardhouse",
}
OFFICIAL_CLASSES = (
    OUTDOOR_CLASSES | RESIDENTIAL_CLASSES | COMMERCIAL_CLASSES
    | INDUSTRIAL_CLASSES | HISTORIC_CLASSES | DEFAULT_CLASSES
)
SUBTYPE_ENVELOPE = {
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
    """Overture (class, subtype, is_underground) -> envelope class 0..5."""
    if underground:
        return 0
    if building_class in OUTDOOR_CLASSES:
        return 0
    if building_class in RESIDENTIAL_CLASSES:
        return 1
    if building_class in COMMERCIAL_CLASSES:
        return 2
    if building_class in INDUSTRIAL_CLASSES:
        return 3
    if building_class in HISTORIC_CLASSES:
        return 4
    # A null or unknown class is not evidence for DEFAULT: class refines
    # subtype, so only that case may use the official subtype fallback.
    if building_class is None or building_class not in OFFICIAL_CLASSES:
        return SUBTYPE_ENVELOPE.get(subtype, ENVELOPE_DEFAULT)
    return ENVELOPE_DEFAULT


def normalize_lon(lon):
    """Fold a longitude into [-180, 180) — the range tile names, stored
    coordinates and H3 lookups all speak."""
    return ((lon + 180.0) % 360.0) - 180.0


def cell_tile_columns(cell, boundary_lons):
    """The 1-degree tile columns a cell covers, as integer longitudes unwrapped
    around the cell centre. Plain min/max longitudes make an antimeridian cell
    span nearly the whole planet: measured 2026-09-04 over the 121,790 prepared
    R4 cells, 65 straddle it and the widest then enumerates 720 tiles instead of
    2 — every one of them a missing-parquet hard fail. Unwrapping fixes 64; the
    remaining one is the South Pole pentagon, which genuinely covers every
    longitude and gets the whole range."""
    centre_lon = h3.cell_to_latlng(cell)[1]
    unwrapped = [lon - 360.0 * round((lon - centre_lon) / 360.0) for lon in boundary_lons]
    if max(unwrapped) - min(unwrapped) > 180.0:
        return range(-180, 180)
    return range(math.floor(min(unwrapped)), math.floor(max(unwrapped)) + 1)


def footprint_centroid(geom):
    """The footprint's centroid as (lat, lon in [-180, 180)). Shapely's centroid
    is planar, so a footprint stored across the antimeridian (+179.99 ..
    -179.99) would centre near 0 deg and be assigned to the wrong tile and cell;
    unwrapping its coordinates around the first one fixes that. A footprint
    narrower than 180 deg — every real one — takes the untouched centroid, so
    the world's rows keep their exact bytes."""
    minimum_lon, _, maximum_lon, _ = geom.bounds
    if maximum_lon - minimum_lon <= 180.0:
        centroid = geom.centroid
        return centroid.y, centroid.x
    reference = float(shapely.get_coordinates(geom)[0][0])
    unwrapped = shapely.transform(
        geom,
        lambda xy: np.column_stack(
            (xy[:, 0] - 360.0 * np.round((xy[:, 0] - reference) / 360.0), xy[:, 1])
        ),
    )
    centroid = unwrapped.centroid
    return centroid.y, normalize_lon(centroid.x)


def read_overture_parquet(parquet_dir, cell):
    """The cell's Overture rows from the one-degree parquets: every 1-degree
    tile the cell's boundary bbox touches, rows kept by the ingest's half-open
    tile-ownership rule (a bbox-overlapping row staged by a neighbouring tile is
    skipped here exactly as the ingest skipped it), then assigned to this cell
    by GEOS centroid — the ingest's ownership rule evaluated per cell.

    Returns (rows, newest parquet mtime) — the freshness stamp of the Overture
    release this cell was built from."""
    boundary = h3.cell_to_boundary(cell)
    lats = [p[0] for p in boundary]
    lat0, lat1 = math.floor(min(lats)), math.floor(max(lats))
    columns = cell_tile_columns(cell, [p[1] for p in boundary])
    rows = []
    newest_mtime = None
    for lat in range(lat0, lat1 + 1):
        for tile_column in columns:
            lon = int(normalize_lon(tile_column))
            name = f"{'N' if lat >= 0 else 'S'}{abs(lat):02d}{'E' if lon >= 0 else 'W'}{abs(lon):03d}"
            src = os.path.join(parquet_dir, f"{name}.parquet")
            if not os.path.exists(src):
                raise SystemExit(
                    f"{cell}: Overture parquet {src} is missing — run "
                    f"scripts/obstacles/download-overture-tiles.py first"
                )
            mtime = os.path.getmtime(src)
            newest_mtime = mtime if newest_mtime is None else max(newest_mtime, mtime)
            pf = pq.ParquetFile(src)
            have = set(pf.schema_arrow.names)
            cols = [c for c in ("geometry", "height", "num_floors", "class",
                                "subtype", "is_underground") if c in have]
            for batch in pf.iter_batches(columns=cols):
                t = pa.Table.from_batches([batch])
                geoms = t.column("geometry").to_pylist()
                n = len(geoms)
                heights = t.column("height").to_pylist() if "height" in have else [None] * n
                floors = t.column("num_floors").to_pylist() if "num_floors" in have else [None] * n
                classes = t.column("class").to_pylist() if "class" in have else [None] * n
                subtypes = t.column("subtype").to_pylist() if "subtype" in have else [None] * n
                und = t.column("is_underground").to_pylist() if "is_underground" in have else [False] * n
                for g, h, f, bc, st, ug in zip(geoms, heights, floors, classes, subtypes, und):
                    if g is None:
                        continue
                    geom = shapely_wkb.loads(bytes(g))
                    if geom.is_empty or geom.geom_type not in ("Polygon", "MultiPolygon"):
                        continue
                    clat, clon = footprint_centroid(geom)
                    if not (math.isfinite(clat) and math.isfinite(clon)):
                        continue
                    # Half-open tile ownership: border footprints appear in both
                    # tiles' downloads; exactly one tile owns them.
                    if not (lat <= clat < lat + 1 and lon <= clon < lon + 1):
                        continue
                    if h3.latlng_to_cell(clat, clon, 4) != cell:
                        continue
                    # The ingest ladder, evaluated here instead of staged.
                    if h is not None and math.isfinite(h) and h > 0:
                        hh, tier = float(h), 0
                    elif f is not None and math.isfinite(f) and f > 0:
                        hh, tier = float(f) * FLOOR_HEIGHT, 1
                    else:
                        hh, tier = DEFAULT_HEIGHT, 2
                    rows.append(
                        {"wkb": bytes(g), "height_m": hh, "tier": tier,
                         "clat": clat, "clon": clon,
                         "envelope": envelope_class(bc, st, ug)}
                    )
    return rows, newest_mtime


# ── The merge ─────────────────────────────────────────────────────────────────

def load_osm_buildings(path):
    if not os.path.exists(path):
        return None
    t = ipc.open_file(path).read_all()
    # The builder propagates building_type ids into the merged table, and the
    # engine gates structures.arrow on structures_contract — so a stale
    # buildings.arrow must be rejected HERE or the contract chain silently
    # certifies someone else's renumbering.
    contract = (t.schema.metadata or {}).get(b"buildings_contract")
    if contract != b"buildings_v2":
        raise SystemExit(
            f"{path}: buildings_contract mismatch (expected buildings_v2, got "
            f"{contract!r}) — re-extract OSM"
        )
    missing = [c for c in BUILDINGS_COLUMNS if c not in t.column_names]
    if missing:
        raise SystemExit(f"{path}: buildings.arrow lacks columns {missing} — re-extract OSM")
    return {c: t.column(c).to_pylist() for c in BUILDINGS_COLUMNS}


def load_barriers(path):
    if not os.path.exists(path):
        return []
    t = ipc.open_file(path).read_all()
    cols = {c: t.column(c).to_pylist()
            for c in ("osm_id", "segment_idx", "start_lat", "start_lon",
                      "end_lat", "end_lon", "height")}
    # height_tier exists on post-2026-09 extracts; older files infer it below.
    if "height_tier" in t.column_names:
        cols["height_tier"] = t.column("height_tier").to_pylist()
    return [dict(zip(cols.keys(), vals)) for vals in zip(*cols.values())]


def wall_wkb(start_lat, start_lon, end_lat, end_lon):
    """2-point little-endian WKB LineString — the wall micro-segment's geometry."""
    return (
        struct.pack("<BI", 1, 2)
        + struct.pack("<I", 2)
        + struct.pack("<dddd", start_lon, start_lat, end_lon, end_lat)
    )


def match_pairs(osm_geoms, osm_geom_idx, overture_rows):
    """One-to-one OSM<->Overture assignment; returns {overture_row: osm_row}.

    Rule: the Overture centroid lies in the OSM polygon OR IoU >= 0.5; greedy by
    (iou desc, centroid_in desc, overture row asc, osm row asc). Deterministic:
    the STRtree is built once in file order and every tie-break is explicit.

    EVERY qualifying pair enters the assignment, not just each Overture row's
    own best candidate: a row whose first choice is taken by a higher-ranked
    pair must fall through to its next qualifying OSM twin. Keeping only the
    local best dropped such a row to Overture-only while its twin stayed
    OSM-only, and one physical structure then screened TWICE — once as the OSM
    polygon, once as the Overture one. Skipping an edge whose Overture row is
    already matched keeps the greedy result identical wherever nothing is
    contested."""
    if not overture_rows or not osm_geoms:
        return {}
    tree = STRtree(osm_geoms)
    edges = []  # (iou, centroid_in, ovt_row, osm_row) — the COMPLETE qualifying set
    for j, row in enumerate(overture_rows):
        g = row.get("geom")
        if g is None:
            g = shapely_wkb.loads(row["wkb"])
            row["geom"] = g
        c = g.centroid
        for k in tree.query(g, predicate="intersects"):
            og = osm_geoms[k]
            contains = og.covers(c)
            try:
                inter = g.intersection(og).area
            except Exception:
                # Invalid rings (self-intersections) break GEOS overlay; the
                # cleaned pair still answers "same building" deterministically.
                gg = g if g.is_valid else g.buffer(0)
                oo = og if og.is_valid else og.buffer(0)
                inter = 0.0 if gg.is_empty or oo.is_empty else gg.intersection(oo).area
            iou = 0.0
            if inter > 0.0:
                union = g.area + og.area - inter
                iou = inter / union if union > 0 else 0.0
            if contains or iou >= IOU_MATCH_THRESHOLD:
                edges.append((iou, 1 if contains else 0, j, osm_geom_idx[k]))
    # Greedy one-to-one: best pairs first; explicit row-order tie-breaks.
    edges.sort(key=lambda e: (-e[0], -e[1], e[2], e[3]))
    matched_ovt, matched_osm, pairs = set(), set(), {}
    for _iou, _centroid_in, j, i in edges:
        if j in matched_ovt or i in matched_osm:
            continue
        matched_ovt.add(j)
        matched_osm.add(i)
        pairs[j] = i
    return pairs


def ladder_osm_only(height_tag, floors):
    """The staging ladder evaluated over OSM tags (no Overture twin exists)."""
    if height_tag is not None and math.isfinite(height_tag) and height_tag > 0:
        return float(height_tag), 0
    if floors:
        return float(floors) * FLOOR_HEIGHT, 1
    return DEFAULT_HEIGHT, 2


def apply_raster_tiers(rows, regional, ghsl, stats):
    """Tiers 3/4 over row dicts keyed (tier, height_m, clat, clon, geom): the
    regional zonal mean replaces tiers 1/2, the ANBH prior only tier 2."""
    n = len(rows)
    if n == 0:
        return
    in_regional = np.zeros(n, dtype=bool)
    if regional is not None:
        rx, ry = regional.tr.transform(
            [r["clon"] for r in rows], [r["clat"] for r in rows]
        )
        for i in range(n):
            in_regional[i] = regional.covers(rx[i], ry[i])
    for i, row in enumerate(rows):
        tier = row["tier"]
        if tier == 0:
            continue
        if in_regional[i]:
            geom = row.get("geom")
            if geom is None:
                geom = shapely_wkb.loads(row["wkb"])
                row["geom"] = geom
            h = regional.zonal_measured_mean(geom)
            if h is not None:
                row["height_m"] = min(max(h, TIER3_CLAMP[0]), TIER3_CLAMP[1])
                row["tier"] = 3
                stats["tier3"] += 1
                continue
            stats["abstain"] += 1
        if tier == 2:
            v = ghsl.sample(row["clon"], row["clat"])
            if v is not None and v >= ANBH_MIN_M:
                row["height_m"] = min(max(v, TIER4_CLAMP[0]), TIER4_CLAMP[1])
                row["tier"] = 4
                stats["tier4"] += 1


def build_cell(cell, h3r4_dir, osm_dir, overture_rows, overture_mtime, ghsl,
               regional, validate):
    """Write one cell's structures.arrow; return the per-cell census dict, or
    None when the cell is up to date (idempotent skip). The two OSM inputs come
    from `osm_dir/<cell>/` (the extract's source tree); the table is written into
    the prepared cell."""
    cell_dir = os.path.join(h3r4_dir, cell)
    if not os.path.isdir(cell_dir):
        raise SystemExit(
            f"{cell}: no prepared cell directory {cell_dir} — the structure table "
            f"follows the prepared inventory and must not extend it"
        )
    osm_cell_dir = os.path.join(osm_dir, cell)
    overture_rows = overture_rows or []
    out_path = os.path.join(cell_dir, "structures.arrow")
    if os.path.exists(out_path):
        out_mtime = os.path.getmtime(out_path)
        inputs = [os.path.join(osm_cell_dir, n)
                  for n in ("buildings.arrow", "barriers.arrow")]
        # EVERY input the row values are computed from, or a refreshed one is
        # served stale for ever: the two per-cell arrows, the Overture source
        # (shard tree or parquet release) and the height-ladder rasters.
        mtimes = [os.path.getmtime(p) for p in inputs if os.path.exists(p)]
        if overture_mtime is not None:
            mtimes.append(overture_mtime)
        mtimes.append(ghsl.mtime)
        if regional is not None:
            mtimes.append(regional.mtime)
        if max(mtimes) <= out_mtime:
            return None  # idempotent: no input is newer than the output
    osm = load_osm_buildings(os.path.join(osm_cell_dir, "buildings.arrow"))
    barriers = load_barriers(os.path.join(osm_cell_dir, "barriers.arrow"))

    # OSM geometry index for matching (rows with a polygon only).
    osm_geoms, osm_geom_idx, osm_geom_by_row = [], [], {}
    if osm is not None:
        for i, w in enumerate(osm["polygon_wkb"]):
            if w is None:
                continue
            g = shapely_wkb.loads(w)
            if g.is_empty:
                continue
            osm_geoms.append(g)
            osm_geom_idx.append(i)
            osm_geom_by_row[i] = g
    pairs = match_pairs(osm_geoms, osm_geom_idx, overture_rows)
    osm_to_ovt = {i: j for j, i in pairs.items()}

    # Ladder: Overture rows ladder from shard/parquet tags; OSM-only rows from
    # OSM tags. Matched rows keep the Overture-side result (module header).
    osm_only = {}
    n_osm = len(osm["osm_id"]) if osm is not None else 0
    matched_osm = set(pairs.values())
    raster_rows = list(overture_rows)
    for i in range(n_osm):
        if i in matched_osm:
            continue
        w = osm["polygon_wkb"][i]
        h, tier = ladder_osm_only(osm["height"][i], osm["floors"][i])
        row = {"wkb": w, "height_m": h, "tier": tier,
               "clat": osm["centroid_lat"][i], "clon": osm["centroid_lon"][i],
               "geom": osm_geom_by_row.get(i), "osm_row": i}
        osm_only[i] = row
        if w is not None:
            raster_rows.append(row)
        # geometry-less OSM row: emission-only; its tag ladder is still the
        # honest height_m, but no raster samples a polygon it does not have.
    stats = {"tier3": 0, "tier4": 0, "abstain": 0}
    apply_raster_tiers(raster_rows, regional, ghsl, stats)

    # Compose the table: OSM rows in buildings.arrow order (emission identity),
    # then Overture-only rows in source order, then walls in file order.
    out = {f: [] for f in SCHEMA.names}
    n_both = 0
    # Index insertion order (screening_ordinal): Overture-stock rows keep their
    # source position j (the Overture stock's own order); OSM-only rows with
    # geometry follow in buildings.arrow order; walls last, in file order.
    n_osm_only_geom = sum(
        1 for i in osm_only if osm["polygon_wkb"][i] is not None
    )
    osm_only_geom_counter = 0
    wall_counter = 0

    def emit(i_osm, ovt, ordinal):
        if ovt is not None:
            geom_wkb = ovt["wkb"]
            height_m, tier = ovt["height_m"], ovt["tier"]
            envelope = ovt["envelope"]
            clat, clon = ovt["clat"], ovt["clon"]
        else:
            geom_wkb = osm["polygon_wkb"][i_osm]
            r = osm_only[i_osm]  # every unmatched OSM row laddered above
            height_m, tier = r["height_m"], r["tier"]
            envelope = ENVELOPE_FROM_BUILDING_USE.get(
                osm["building_use"][i_osm], ENVELOPE_DEFAULT
            )
            clat, clon = osm["centroid_lat"][i_osm], osm["centroid_lon"][i_osm]
        out["kind"].append(KIND_BUILDING)
        out["geometry_wkb"].append(geom_wkb)
        out["height_m"].append(height_m)
        out["height_tier"].append(tier)
        out["envelope_class"].append(envelope)
        out["centroid_lat"].append(clat)
        out["centroid_lon"].append(clon)
        for c in ("osm_id", "building_type", "building_use", "height", "floors",
                  "name", "addr_street", "addr_housenumber", "area_m2",
                  "opening_hours_frac", "source_id"):
            out[c].append(osm[c][i_osm] if i_osm is not None else None)
        emission_poly = None
        if i_osm is not None:
            osm_wkb = osm["polygon_wkb"][i_osm]
            area = osm["area_m2"][i_osm]
            needs_poly = osm_wkb is not None and (
                area is None or not (area > 0.0) or area > EMISSION_GRID_THRESHOLD_M2
            )
            if needs_poly and osm_wkb != geom_wkb:
                emission_poly = osm_wkb
        out["emission_polygon_wkb"].append(emission_poly)
        if i_osm is not None and ovt is not None:
            out["emission_centroid_lat"].append(osm["centroid_lat"][i_osm])
            out["emission_centroid_lon"].append(osm["centroid_lon"][i_osm])
        else:
            out["emission_centroid_lat"].append(None)
            out["emission_centroid_lon"].append(None)
        out["segment_idx"].append(None)
        out["screening_ordinal"].append(ordinal)

    for i in range(n_osm):
        j = osm_to_ovt.get(i)
        if j is not None:
            n_both += 1
            emit(i, overture_rows[j], j)
        else:
            has_geom = osm["polygon_wkb"][i] is not None
            ordinal = None
            if has_geom:
                ordinal = len(overture_rows) + osm_only_geom_counter
                osm_only_geom_counter += 1
            emit(i, None, ordinal)
    matched_ovt = set(pairs.keys())
    n_ovt_only = 0
    for j, row in enumerate(overture_rows):
        if j in matched_ovt:
            continue
        n_ovt_only += 1
        emit(None, row, j)

    # Walls: one row per micro-segment, polyline WKB, mapped-or-default height.
    n_wall_tier_inferred = 0
    for b in barriers:
        out["kind"].append(KIND_BARRIER)
        out["geometry_wkb"].append(
            wall_wkb(b["start_lat"], b["start_lon"], b["end_lat"], b["end_lon"])
        )
        h = b["height"]
        out["height_m"].append(h)
        # barriers.arrow carries the spill's tier on new extracts; the 2026
        # world's files predate it, so the migration infers from the value (a
        # genuinely mapped 3.0 m wall marked tier 2 is harmless: nothing caps
        # or ladders kind=barrier rows).
        tier = b.get("height_tier")
        if tier is None:
            tier = 2 if h == WALL_DEFAULT_HEIGHT_M else 0
            n_wall_tier_inferred += 1
        out["height_tier"].append(tier)
        out["envelope_class"].append(ENVELOPE_OUTDOOR)
        out["centroid_lat"].append((b["start_lat"] + b["end_lat"]) / 2.0)
        out["centroid_lon"].append((b["start_lon"] + b["end_lon"]) / 2.0)
        out["osm_id"].append(b["osm_id"])
        for c in ("building_type", "building_use", "height", "floors", "name",
                  "addr_street", "addr_housenumber", "area_m2",
                  "opening_hours_frac", "source_id", "emission_polygon_wkb",
                  "emission_centroid_lat", "emission_centroid_lon"):
            out[c].append(None)
        out["segment_idx"].append(b["segment_idx"])
        out["screening_ordinal"].append(
            len(overture_rows) + n_osm_only_geom + wall_counter
        )
        wall_counter += 1

    meta = dict(SCHEMA.metadata or {})
    meta[CONTRACT_KEY.encode()] = CONTRACT_VERSION.encode()
    meta[b"building_rows"] = str(n_osm + n_ovt_only).encode()
    meta[b"barrier_rows"] = str(len(barriers)).encode()
    schema = SCHEMA.with_metadata(meta)
    table = pa.table(out, schema=schema)

    if validate:
        validate_cell(cell, osm, table)

    tmp = f"{out_path}.tmp.{os.getpid()}"
    with ipc.new_file(tmp, schema) as w:
        # Sequential 4096-row chunks, no spatial re-sort: the emission stream is
        # the buildings.arrow subsequence and must not be reordered.
        for batch in table.to_batches(max_chunksize=4096):
            w.write_batch(batch)
    fd = os.open(tmp, os.O_RDONLY)
    try:
        os.fsync(fd)
        os.replace(tmp, out_path)
        dir_fd = os.open(cell_dir, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(dir_fd)
        finally:
            os.close(dir_fd)
    finally:
        os.close(fd)
    return {
        "cell": cell,
        "osm_rows": n_osm,
        "both": n_both,
        "osm_only": n_osm - n_both,
        "overture_only": n_ovt_only,
        "walls": len(barriers),
        "rows": table.num_rows,
        "tier3": stats["tier3"],
        "tier4": stats["tier4"],
        "regional_abstain": stats["abstain"],
        "wall_tier_inferred": n_wall_tier_inferred,
        "bytes": os.path.getsize(out_path),
    }


def validate_cell(cell, osm, table):
    """Emission-view proof for one cell (raises, never warns): the emission view
    (kind=0, osm_id present, file order) equals buildings.arrow row by row on
    every emission column, with the emission polygon = emission_polygon_wkb ??
    geometry_wkb and the emission centroid = emission_centroid_* ?? centroid_*."""
    if osm is not None:
        mask = pc.and_(
            pc.equal(table.column("kind"), KIND_BUILDING),
            pc.is_valid(table.column("osm_id")),
        )
        view = table.filter(mask)
        n = len(osm["osm_id"])
        if view.num_rows != n:
            raise SystemExit(
                f"{cell}: emission view rows {view.num_rows} != buildings.arrow {n}"
            )
        cols = {c: view.column(c).to_pylist() for c in EMISSION_COMPARE}
        epoly = view.column("emission_polygon_wkb").to_pylist()
        geom = view.column("geometry_wkb").to_pylist()
        eclat = view.column("emission_centroid_lat").to_pylist()
        eclon = view.column("emission_centroid_lon").to_pylist()
        clat = view.column("centroid_lat").to_pylist()
        clon = view.column("centroid_lon").to_pylist()
        for i in range(n):
            for c in EMISSION_COMPARE:
                if cols[c][i] != osm[c][i]:
                    raise SystemExit(
                        f"{cell}: emission row {i} column {c}: "
                        f"{cols[c][i]!r} != {osm[c][i]!r}"
                    )
            # The polygon enters emission only where the loader can read it:
            # area missing (shoelace fallback) or above the grid-split threshold
            # (noise_compute::normalize::points). Below it the point stream is
            # polygon-independent by construction, and the sparse
            # emission_polygon_wkb stays null.
            area = osm["area_m2"][i]
            if (area is None or not (area > 0.0) or area > EMISSION_GRID_THRESHOLD_M2) and (
                epoly[i] or geom[i]
            ) != osm["polygon_wkb"][i]:
                raise SystemExit(f"{cell}: emission row {i} polygon differs")
            if (eclat[i] if eclat[i] is not None else clat[i]) != osm["centroid_lat"][i]:
                raise SystemExit(f"{cell}: emission row {i} centroid_lat differs")
            if (eclon[i] if eclon[i] is not None else clon[i]) != osm["centroid_lon"][i]:
                raise SystemExit(f"{cell}: emission row {i} centroid_lon differs")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--h3r4-dir", required=True)
    ap.add_argument("--osm-dir", required=True,
                    help="OSM extract tree: <cell>/buildings.arrow + <cell>/barriers.arrow")
    src = ap.add_mutually_exclusive_group(required=True)
    src.add_argument("--overture-shards")
    src.add_argument("--overture-parquet")
    ap.add_argument("--ghsl", required=True)
    ap.add_argument("--regional")
    group = ap.add_mutually_exclusive_group(required=True)
    group.add_argument("--cells")
    group.add_argument("--cells-file")
    ap.add_argument("--validate", action="store_true",
                    help="prove the emission view against buildings.arrow, cell by cell")
    ap.add_argument("--census-log", help="append one JSON line per built cell (the world migration's count proof)")
    args = ap.parse_args()

    cells = ([line.strip() for line in open(args.cells_file) if line.strip()]
             if args.cells_file else args.cells.split(","))
    ghsl = GlobalPrior(args.ghsl)
    regional = RegionalHeights(args.regional) if args.regional else None
    census_log = open(args.census_log, "a", encoding="utf-8") if args.census_log else None
    totals = {"built": 0, "fresh_skip": 0, "osm_only": 0, "both": 0,
              "overture_only": 0, "walls": 0, "rows": 0, "bytes": 0}
    for done, cell in enumerate(cells, start=1):
        if args.overture_shards:
            ovt, ovt_mtime = read_overture_shards(os.path.join(args.overture_shards, cell))
        else:
            ovt, ovt_mtime = read_overture_parquet(args.overture_parquet, cell)
        census = build_cell(cell, args.h3r4_dir, args.osm_dir, ovt, ovt_mtime,
                            ghsl, regional, args.validate)
        if census is None:
            totals["fresh_skip"] += 1
        else:
            totals["built"] += 1
            for k in ("osm_only", "both", "overture_only", "walls", "rows", "bytes"):
                totals[k] += census[k]
            if census_log is not None:
                census_log.write(json.dumps(census) + "\n")
                census_log.flush()
        if done % 1000 == 0 or done == len(cells):
            print(
                f"[build-structures] {done}/{len(cells)}: built={totals['built']} "
                f"fresh-skip={totals['fresh_skip']} both={totals['both']} "
                f"osm-only={totals['osm_only']} overture-only={totals['overture_only']} "
                f"walls={totals['walls']} rows={totals['rows']} bytes={totals['bytes']}",
                flush=True,
            )
    if census_log is not None:
        census_log.close()
    print(f"[build-structures] DONE {totals}", flush=True)


if __name__ == "__main__":
    main()
