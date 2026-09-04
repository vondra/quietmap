#!/usr/bin/env python3
# build-structures.py — write the ONE per-square structure table: structures.arrow.
#
# Every prepared z9 square holds structures.arrow — merged where anything stands,
# a 0-row table with the schema below where nothing does. One row per physical
# structure carries the union of:
#   * OSM buildings (buildings.arrow as osm-extract emits it) — the EMISSION
#     stock: attributes and grid polygons drive the building layer;
#   * Overture footprints (the screening stock) — a matched OSM<->Overture pair
#     shares one row; an Overture footprint with no OSM twin is attribute-less;
#   * OSM noise walls (barriers.arrow) as kind=barrier polyline micro-segments.
#
# Semantics contract (engine readers rely on it):
#   * emission reads kind=0 rows with osm_id present, in file order — exactly
#     today's buildings.arrow subsequence with the same values, so the building
#     layer is unchanged; the emission polygon is emission_geom ?? geom, stored
#     only where emission can read it (area missing or > 2000 m2) and different
#     from the screening polygon; the emission position is emission_centroid_*
#     ?? centroid_*;
#   * screening reads every row with geometry; matched pairs keep the OVERTURE
#     polygon (census 2026-09-03: 0 of 2.83 M matched pairs share geometry);
#   * walls screen as Barrier polylines inside the same index;
#   * airborne reads kind=0 polygons and ignores barriers, exactly as today.
#
# Matching rule: an Overture footprint matches an OSM building iff the Overture
# centroid lies in the OSM polygon OR IoU >= 0.5; greedy one-to-one by
# (iou desc, centroid_in desc, overture row asc, osm row asc).
#
# Height ladder (tier semantics are load-time contract in the future
# noise-compute low_profile, which caps tiers 2/4 to 3 m next to small OSM
# buildings — kind=0 rows only):
#   tier 0 mapped height · 1 floors x 3 m · 2 flat 8 m default ·
#   3 regional measured zonal (replaces 1/2) · 4 GHSL ANBH prior (replaces 2 only)
# --validate proves the emission view against buildings.arrow row by row, so a
# merge that would move the building layer fails the square instead of painting it.
#
# Per square: deterministic, idempotent (rebuild iff an input is newer than the
# output), re-runnable in any order, atomic (tmp+fsync+rename, dir fsync), never
# creates a prepared square directory (the table follows the prepared inventory).
#
# Usage:
#   build-structures.py --prepared-dir data/prepared/2026 \
#     --overture-parquet DIR --ghsl <ANBH.tif> [--regional <mosaic.vrt>] \
#     (--squares z9/276/173,... | --squares-file F) [--validate]
import argparse
import json
import math
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "lib"))
import qmgrid

import numpy as np
import pyarrow as pa
import pyarrow.compute as pc
import pyarrow.ipc as ipc
import pyarrow.parquet as pq
import rasterio
import shapely
import shapely.ops
from pyproj import Transformer
from shapely import STRtree

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
ENVELOPE_FROM_BUILDING_USE = {0: 1, 1: 2, 2: 3}

IOU_MATCH_THRESHOLD = 0.5

CONTRACT_KEY = "structures_contract"
CONTRACT_VERSION = "structures_v2"  # v1 was float lat/lon + WKB; v2 is int32 grid

SCHEMA = pa.schema(
    [
        pa.field("kind", pa.uint8(), nullable=False),
        # Snapped grid polygon (qmgrid.encode_grid_poly form), screening stock.
        pa.field("geom", pa.binary()),
        pa.field("height_m", pa.float32(), nullable=False),
        pa.field("height_tier", pa.uint8(), nullable=False),
        pa.field("envelope_class", pa.uint8(), nullable=False),
        pa.field("centroid_gx", pa.int32(), nullable=False),
        pa.field("centroid_gy", pa.int32(), nullable=False),
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
        # Emission overrides (null -> geom / centroid_*).
        pa.field("emission_geom", pa.binary()),
        pa.field("emission_centroid_gx", pa.int32()),
        pa.field("emission_centroid_gy", pa.int32()),
        # Wall micro-segment index (barrier rows only).
        pa.field("segment_idx", pa.int16()),
        # Obstacle-index insertion order (see the proven v1 comment: builders
        # assign it, loaders sort by it, null = never indexed).
        pa.field("screening_ordinal", pa.uint32()),
    ]
)

BUILDINGS_COLUMNS = [
    "osm_id", "centroid_gx", "centroid_gy", "building_type", "building_use",
    "height", "floors", "name", "addr_street", "addr_housenumber", "geom",
    "area_m2", "opening_hours_frac", "source_id",
]

# The columns the emission view is validated against, in buildings.arrow order.
EMISSION_COMPARE = [
    "osm_id", "building_type", "building_use", "height", "floors", "name",
    "addr_street", "addr_housenumber", "area_m2", "opening_hours_frac",
    "source_id",
]


# ── Raster height sources (rasterio; wheels bundle GDAL) ─────────────────────

def raster_mtime(ds):
    """Newest mtime over every file the raster is made of."""
    return max(os.path.getmtime(f) for f in ds.files)


class GlobalPrior:
    """GHS-BUILT-H ANBH: nearest-pixel value at a WGS84 point (windowed reads)."""

    def __init__(self, path):
        self.ds = rasterio.open(path)
        self.gt = self.ds.transform
        self.w, self.h = self.ds.width, self.ds.height
        self.crs = self.ds.crs
        if self.crs is None:
            raise SystemExit(f"{path}: raster is not georeferenced — re-fetch it")
        self.tr = Transformer.from_crs("EPSG:4326", self.crs, always_xy=True)
        self.mtime = raster_mtime(self.ds)

    def sample(self, lon, lat):
        x, y = self.tr.transform(lon, lat)
        ci = int((x - self.gt.c) / self.gt.a)
        ri = int((y - self.gt.f) / self.gt.e)
        if not (0 <= ci < self.w and 0 <= ri < self.h):
            return None
        v = float(self.ds.read(1, window=((ri, ri + 1), (ci, ci + 1)))[0, 0])
        nodata = self.ds.nodata
        if not math.isfinite(v) or v >= ANBH_MAX_VALID:
            return None
        if nodata is not None and v == nodata:
            return None
        return v


class RegionalHeights:
    """Regional relative-height raster held fully in RAM (zonal reads cluster)."""

    def __init__(self, path):
        ds = rasterio.open(path)
        self.gt = ds.transform
        self.w, self.h = ds.width, ds.height
        self.tr = Transformer.from_crs("EPSG:4326", ds.crs, always_xy=True)
        self.mtime = raster_mtime(ds)
        self.arr = ds.read(1).astype(np.float32, copy=False)
        nodata = ds.nodata
        if nodata is not None:
            self.arr[self.arr == nodata] = np.nan

    def covers(self, x, y):
        c = (x - self.gt.c) / self.gt.a
        r = (y - self.gt.f) / self.gt.e
        return 0 <= c < self.w and 0 <= r < self.h

    def zonal_measured_mean(self, geom_wgs84):
        """Mean of in-footprint pixels >= MEASURED_MIN_M, or None when the
        coverage guard says the city model does not know this structure."""
        g = shapely.ops.transform(self.tr.transform, geom_wgs84)
        minx, miny, maxx, maxy = g.bounds
        c0 = max(0, int(math.floor((minx - self.gt.c) / self.gt.a)))
        c1 = min(self.w, int(math.ceil((maxx - self.gt.c) / self.gt.a)) + 1)
        r0 = max(0, int(math.floor((maxy - self.gt.f) / self.gt.e)))
        r1 = min(self.h, int(math.ceil((miny - self.gt.f) / self.gt.e)) + 1)
        if c1 <= c0 or r1 <= r0:
            return None
        # A malformed continent-scale footprint would mesh-grid gigabytes here
        # (gg pass 2) — no real building needs a 4x4 km window; abstain.
        if (c1 - c0) * (r1 - r0) > 16_000_000:
            return None
        window = self.arr[r0:r1, c0:c1]
        xs = self.gt.c + (np.arange(c0, c1) + 0.5) * self.gt.a
        ys = self.gt.f + (np.arange(r0, r1) + 0.5) * self.gt.e
        xx, yy = np.meshgrid(xs, ys)
        inside = shapely.contains_xy(g, xx.ravel(), yy.ravel()).reshape(window.shape)
        vals = window[inside]
        vals = vals[np.isfinite(vals)]
        measured = vals[vals >= MEASURED_MIN_M]
        if len(measured) < max(COVERAGE_MIN_PX, COVERAGE_MIN_FRAC * int(inside.sum())):
            return None
        return float(measured.mean())


# ── Overture row sources ──────────────────────────────────────────────────────

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
    if building_class is None or building_class not in OFFICIAL_CLASSES:
        return SUBTYPE_ENVELOPE.get(subtype, ENVELOPE_DEFAULT)
    return ENVELOPE_DEFAULT


def normalize_lon(lon):
    return ((lon + 180.0) % 360.0) - 180.0


def footprint_centroid(geom):
    """The footprint's centroid as (lat, lon in [-180, 180)). Antimeridian
    unwrap around the first coordinate (proven form — kept verbatim)."""
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


def grid_ring_to_shapely(ring):
    """Snapped grid ring -> shapely Polygon in lon/lat (matching geometry)."""
    return shapely.Polygon(qmgrid.ring_to_lonlat(ring))


def overture_height_ladder(h, f):
    """The ingest ladder: mapped height, floors x 3 m, else the 8 m default."""
    if h is not None and math.isfinite(h) and h > 0:
        return float(h), 0
    if f is not None and math.isfinite(f) and f > 0:
        return float(f) * FLOOR_HEIGHT, 1
    return DEFAULT_HEIGHT, 2


def read_overture_parquet(parquet_dir, square):
    """The square's Overture rows from the one-degree parquets: every 1-degree
    tile the square's span touches, rows kept by the half-open tile-ownership
    rule, then assigned to this square by GEOS centroid. A z9 square never
    straddles the antimeridian (spans slice [-180, 180)), so no unwrapping.

    Returns (rows, newest parquet mtime)."""
    from shapely import wkb as shapely_wkb

    x, y = square
    lon0, lat_top, lon1, lat_bot = qmgrid.square_lonlat_span(x, y)
    rows = []
    newest_mtime = None
    for lat in range(math.floor(lat_bot), math.floor(lat_top) + 1):
        for lon in range(math.floor(lon0), math.floor(lon1) + 1):
            name = f"{'N' if lat >= 0 else 'S'}{abs(lat):02d}{'E' if lon >= 0 else 'W'}{abs(lon):03d}"
            src = os.path.join(parquet_dir, f"{name}.parquet")
            if not os.path.exists(src):
                raise SystemExit(
                    f"{qmgrid.square_name(x, y)}: Overture parquet {src} is missing — run "
                    f"scripts/overture/download-overture-world.sh first"
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
                    if not (lat <= clat < lat + 1 and lon <= clon < lon + 1):
                        continue
                    if qmgrid.square_of(clat, clon) != (x, y):
                        continue
                    hh, tier = overture_height_ladder(h, f)
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
    contract = (t.schema.metadata or {}).get(b"buildings_contract")
    if contract != b"buildings_v3":
        raise SystemExit(
            f"{path}: buildings_contract mismatch (expected buildings_v3, got "
            f"{contract!r}) — re-extract OSM"
        )
    grid_pin = (t.schema.metadata or {}).get(b"grid")
    if grid_pin != b"z30":
        raise SystemExit(f"{path}: grid pin mismatch (expected z30, got {grid_pin!r})")
    missing = [c for c in BUILDINGS_COLUMNS if c not in t.column_names]
    if missing:
        raise SystemExit(f"{path}: buildings.arrow lacks columns {missing} — re-extract OSM")
    cols = {c: t.column(c).to_pylist() for c in BUILDINGS_COLUMNS}
    # Snapped grid polygons -> matching geometry (proven GEOS ops below).
    cols["shapely"] = [
        grid_ring_to_shapely(qmgrid.decode_grid_poly(g)) if g is not None else None
        for g in cols["geom"]
    ]
    return cols


def load_barriers(path):
    if not os.path.exists(path):
        return []
    t = ipc.open_file(path).read_all()
    cols = {c: t.column(c).to_pylist()
            for c in ("osm_id", "segment_idx", "start_gx", "start_gy",
                      "end_gx", "end_gy", "height")}
    if "height_tier" in t.column_names:
        cols["height_tier"] = t.column("height_tier").to_pylist()
    return [dict(zip(cols.keys(), vals)) for vals in zip(*cols.values())]


def wall_grid_poly(s_gx, s_gy, e_gx, e_gy):
    """2-point grid polyline — the wall micro-segment's geometry."""
    return qmgrid.encode_grid_poly([(s_gx, s_gy), (e_gx, e_gy)])


def wall_centroid_grid(s_gx, s_gy, e_gx, e_gy):
    lon0, lat0 = qmgrid.grid_to_lonlat(s_gx, s_gy)
    lon1, lat1 = qmgrid.grid_to_lonlat(e_gx, e_gy)
    return qmgrid.lonlat_to_grid((lon0 + lon1) / 2.0, (lat0 + lat1) / 2.0)


def match_pairs(osm_geoms, osm_geom_idx, overture_rows):
    """One-to-one OSM<->Overture assignment; returns {overture_row: osm_row}.
    Proven form (complete qualifying set, greedy by iou/centroid/rows) — kept
    verbatim, geometry source agnostic."""
    from shapely import wkb as shapely_wkb

    if not overture_rows or not osm_geoms:
        return {}
    tree = STRtree(osm_geoms)
    edges = []
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
                gg = g if g.is_valid else g.buffer(0)
                oo = og if og.is_valid else og.buffer(0)
                inter = 0.0 if gg.is_empty or oo.is_empty else gg.intersection(oo).area
            iou = 0.0
            if inter > 0.0:
                union = g.area + og.area - inter
                iou = inter / union if union > 0 else 0.0
            if contains or iou >= IOU_MATCH_THRESHOLD:
                edges.append((iou, 1 if contains else 0, j, osm_geom_idx[k]))
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
                from shapely import wkb as shapely_wkb
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


def build_square(name, prepared_dir, overture_rows, overture_mtime, ghsl, regional, validate):
    """Write one square's structures.arrow; return the census dict, or None
    when the square is up to date (idempotent skip)."""
    x, y = qmgrid.parse_square_name(name)
    square_dir = os.path.join(prepared_dir, "z9", str(x), str(y))
    if not os.path.isdir(square_dir):
        raise SystemExit(
            f"{name}: no prepared square directory {square_dir} — the structure table "
            f"follows the prepared inventory and must not extend it"
        )
    overture_rows = overture_rows or []
    out_path = os.path.join(square_dir, "structures.arrow")
    if os.path.exists(out_path):
        out_mtime = os.path.getmtime(out_path)
        inputs = [os.path.join(square_dir, n) for n in ("buildings.arrow", "barriers.arrow")]
        mtimes = [os.path.getmtime(p) for p in inputs if os.path.exists(p)]
        if overture_mtime is not None:
            mtimes.append(overture_mtime)
        mtimes.append(ghsl.mtime)
        if regional is not None:
            mtimes.append(regional.mtime)
        if max(mtimes) <= out_mtime:
            return None  # idempotent: no input is newer than the output
    osm = load_osm_buildings(os.path.join(square_dir, "buildings.arrow"))
    barriers = load_barriers(os.path.join(square_dir, "barriers.arrow"))

    osm_geoms, osm_geom_idx, osm_geom_by_row = [], [], {}
    if osm is not None:
        for i, g in enumerate(osm["shapely"]):
            if g is None or g.is_empty:
                continue
            osm_geoms.append(g)
            osm_geom_idx.append(i)
            osm_geom_by_row[i] = g
    pairs = match_pairs(osm_geoms, osm_geom_idx, overture_rows)
    osm_to_ovt = {i: j for j, i in pairs.items()}

    osm_only = {}
    n_osm = len(osm["osm_id"]) if osm is not None else 0
    matched_osm = set(pairs.values())
    raster_rows = list(overture_rows)
    for i in range(n_osm):
        if i in matched_osm:
            continue
        h, tier = ladder_osm_only(osm["height"][i], osm["floors"][i])
        gx, gy = osm["centroid_gx"][i], osm["centroid_gy"][i]
        clon, clat = qmgrid.grid_to_lonlat(gx, gy)
        row = {"height_m": h, "tier": tier,
               "clat": clat, "clon": clon, "osm_row": i}
        row["geom"] = osm_geom_by_row.get(i)
        osm_only[i] = row
        if row["geom"] is not None:
            raster_rows.append(row)
    stats = {"tier3": 0, "tier4": 0, "abstain": 0}
    apply_raster_tiers(raster_rows, regional, ghsl, stats)

    out = {f: [] for f in SCHEMA.names}
    n_both = 0
    n_osm_only_geom = sum(
        1 for i in osm_only if osm_geom_by_row.get(i) is not None
    )
    osm_only_geom_counter = 0
    wall_counter = 0

    def snap_geom(geom):
        """Shapely polygon -> snapped grid ring (exterior of the largest part).
        v2 stores one ring per row; holes were already dropped at extract."""
        if geom.geom_type == "MultiPolygon":
            geom = max(geom.geoms, key=lambda p: p.area)
        return [qmgrid.lonlat_to_grid(x, y)
                for x, y in shapely.get_coordinates(geom.exterior)]

    def emit(i_osm, ovt, ordinal):
        if ovt is not None:
            ring = snap_geom(ovt_geom(ovt))
            geom_blob = qmgrid.encode_grid_poly(ring)
            height_m, tier = ovt["height_m"], ovt["tier"]
            envelope = ovt["envelope"]
            cgx, cgy = qmgrid.lonlat_to_grid(ovt["clon"], ovt["clat"])
        else:
            geom_blob = osm["geom"][i_osm]
            r = osm_only[i_osm]  # every unmatched OSM row laddered above
            height_m, tier = r["height_m"], r["tier"]
            envelope = ENVELOPE_FROM_BUILDING_USE.get(
                osm["building_use"][i_osm], ENVELOPE_DEFAULT
            )
            cgx, cgy = osm["centroid_gx"][i_osm], osm["centroid_gy"][i_osm]
        out["kind"].append(KIND_BUILDING)
        out["geom"].append(geom_blob)
        out["height_m"].append(height_m)
        out["height_tier"].append(tier)
        out["envelope_class"].append(envelope)
        out["centroid_gx"].append(cgx)
        out["centroid_gy"].append(cgy)
        for c in ("osm_id", "building_type", "building_use", "height", "floors",
                  "name", "addr_street", "addr_housenumber", "area_m2",
                  "opening_hours_frac", "source_id"):
            out[c].append(osm[c][i_osm] if i_osm is not None else None)
        emission_geom = None
        if i_osm is not None:
            osm_blob = osm["geom"][i_osm]
            area = osm["area_m2"][i_osm]
            needs_poly = osm_blob is not None and (
                area is None or not (area > 0.0) or area > EMISSION_GRID_THRESHOLD_M2
            )
            if needs_poly and osm_blob != geom_blob:
                emission_geom = osm_blob
        out["emission_geom"].append(emission_geom)
        if i_osm is not None and ovt is not None:
            out["emission_centroid_gx"].append(osm["centroid_gx"][i_osm])
            out["emission_centroid_gy"].append(osm["centroid_gy"][i_osm])
        else:
            out["emission_centroid_gx"].append(None)
            out["emission_centroid_gy"].append(None)
        out["segment_idx"].append(None)
        out["screening_ordinal"].append(ordinal)

    def ovt_geom(ovt):
        g = ovt.get("geom")
        if g is None:
            from shapely import wkb as shapely_wkb
            g = shapely_wkb.loads(ovt["wkb"])
            ovt["geom"] = g
        return g

    for i in range(n_osm):
        j = osm_to_ovt.get(i)
        if j is not None:
            n_both += 1
            emit(i, overture_rows[j], j)
        else:
            has_geom = osm_geom_by_row.get(i) is not None
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

    # Walls: one row per micro-segment, grid polyline, mapped-or-default height.
    n_wall_tier_inferred = 0
    for b in barriers:
        out["kind"].append(KIND_BARRIER)
        out["geom"].append(wall_grid_poly(
            b["start_gx"], b["start_gy"], b["end_gx"], b["end_gy"]))
        h = b["height"]
        out["height_m"].append(h)
        tier = b.get("height_tier")
        if tier is None:
            tier = 2 if h == WALL_DEFAULT_HEIGHT_M else 0
            n_wall_tier_inferred += 1
        out["height_tier"].append(tier)
        out["envelope_class"].append(ENVELOPE_OUTDOOR)
        cgx, cgy = wall_centroid_grid(
            b["start_gx"], b["start_gy"], b["end_gx"], b["end_gy"])
        out["centroid_gx"].append(cgx)
        out["centroid_gy"].append(cgy)
        out["osm_id"].append(b["osm_id"])
        for c in ("building_type", "building_use", "height", "floors", "name",
                  "addr_street", "addr_housenumber", "area_m2",
                  "opening_hours_frac", "source_id", "emission_geom",
                  "emission_centroid_gx", "emission_centroid_gy"):
            out[c].append(None)
        out["segment_idx"].append(b["segment_idx"])
        out["screening_ordinal"].append(
            len(overture_rows) + n_osm_only_geom + wall_counter
        )
        wall_counter += 1

    meta = dict(SCHEMA.metadata or {})
    meta[CONTRACT_KEY] = CONTRACT_VERSION
    meta["grid"] = "z30"
    meta["building_rows"] = str(n_osm + n_ovt_only)
    meta["barrier_rows"] = str(len(barriers))
    schema = SCHEMA.with_metadata(meta)
    table = pa.table(out, schema=schema)

    if validate:
        validate_square(name, osm, table)

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
        dir_fd = os.open(square_dir, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(dir_fd)
        finally:
            os.close(dir_fd)
    finally:
        os.close(fd)
    return {
        "square": name,
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


def validate_square(name, osm, table):
    """Emission-view proof for one square (raises, never warns): the emission
    view (kind=0, osm_id present, file order) equals buildings.arrow row by row
    on every emission column, with the emission polygon = emission_geom ??
    geom and the emission centroid = emission_centroid_* ?? centroid_*."""
    if osm is not None:
        mask = pc.and_(
            pc.equal(table.column("kind"), KIND_BUILDING),
            pc.is_valid(table.column("osm_id")),
        )
        view = table.filter(mask)
        n = len(osm["osm_id"])
        if view.num_rows != n:
            raise SystemExit(
                f"{name}: emission view rows {view.num_rows} != buildings.arrow {n}"
            )
        cols = {c: view.column(c).to_pylist() for c in EMISSION_COMPARE}
        egeom = view.column("emission_geom").to_pylist()
        geom = view.column("geom").to_pylist()
        egx = view.column("emission_centroid_gx").to_pylist()
        egy = view.column("emission_centroid_gy").to_pylist()
        cgx = view.column("centroid_gx").to_pylist()
        cgy = view.column("centroid_gy").to_pylist()
        for i in range(n):
            for c in EMISSION_COMPARE:
                if cols[c][i] != osm[c][i]:
                    raise SystemExit(
                        f"{name}: emission row {i} column {c}: "
                        f"{cols[c][i]!r} != {osm[c][i]!r}"
                    )
            area = osm["area_m2"][i]
            if (area is None or not (area > 0.0) or area > EMISSION_GRID_THRESHOLD_M2) and (
                egeom[i] or geom[i]
            ) != osm["geom"][i]:
                raise SystemExit(f"{name}: emission row {i} polygon differs")
            if (egx[i] if egx[i] is not None else cgx[i]) != osm["centroid_gx"][i]:
                raise SystemExit(f"{name}: emission row {i} centroid_gx differs")
            if (egy[i] if egy[i] is not None else cgy[i]) != osm["centroid_gy"][i]:
                raise SystemExit(f"{name}: emission row {i} centroid_gy differs")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--prepared-dir", required=True)
    ap.add_argument("--overture-parquet", required=True)
    ap.add_argument("--ghsl", required=True)
    ap.add_argument("--regional")
    group = ap.add_mutually_exclusive_group(required=True)
    group.add_argument("--squares")
    group.add_argument("--squares-file")
    ap.add_argument("--validate", action="store_true",
                    help="prove the emission view against buildings.arrow, square by square")
    ap.add_argument("--census-log", help="append one JSON line per built square")
    args = ap.parse_args()

    squares = ([line.strip() for line in open(args.squares_file) if line.strip()]
               if args.squares_file else args.squares.split(","))
    for s in squares:
        if qmgrid.parse_square_name(s) is None:
            ap.error(f"not a square name: {s}")
    ghsl = GlobalPrior(args.ghsl)
    regional = RegionalHeights(args.regional) if args.regional else None
    census_log = open(args.census_log, "a", encoding="utf-8") if args.census_log else None
    totals = {"built": 0, "fresh_skip": 0, "osm_only": 0, "both": 0,
              "overture_only": 0, "walls": 0, "rows": 0, "bytes": 0}
    for done, name in enumerate(squares, start=1):
        x, y = qmgrid.parse_square_name(name)
        ovt, ovt_mtime = read_overture_parquet(args.overture_parquet, (x, y))
        census = build_square(name, args.prepared_dir, ovt, ovt_mtime, ghsl, regional,
                              args.validate)
        if census is None:
            totals["fresh_skip"] += 1
        else:
            totals["built"] += 1
            for k in ("osm_only", "both", "overture_only", "walls", "rows", "bytes"):
                totals[k] += census[k]
            if census_log is not None:
                census_log.write(json.dumps(census) + "\n")
                census_log.flush()
        if done % 1000 == 0 or done == len(squares):
            print(
                f"[build-structures] {done}/{len(squares)}: built={totals['built']} "
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
