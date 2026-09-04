"""Raster height sources and canonical Overture footprint ingestion."""

import math
import os

import numpy as np
import pyarrow as pa
import pyarrow.parquet as pq
import rasterio
import shapely
import shapely.ops
from pyproj import Transformer

import qmgrid

MEASURED_MIN_M = 2.0      # zonal pixels below this are "not a building surface here"
COVERAGE_MIN_FRAC = 0.30  # measured pixels must cover this share of the footprint
COVERAGE_MIN_PX = 3
TIER3_CLAMP = (2.5, 250.0)
ANBH_MIN_M = 1.0          # ANBH below this = no better info than the default
ANBH_MAX_VALID = 250.0    # GHSL NoData sentinel is 255 — belt for a missing tag
TIER4_CLAMP = (3.0, 100.0)

FLOOR_HEIGHT = 3.0        # == noise_compute::constants::BUILDING_FLOOR_HEIGHT_M
DEFAULT_HEIGHT = 8.0      # == noise_compute::constants::BUILDING_DEFAULT_HEIGHT_M

ENVELOPE_OUTDOOR = 0
ENVELOPE_DEFAULT = 5
ENVELOPE_FROM_BUILDING_USE = {0: 1, 1: 2, 2: 3}

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
    return centroid.y, qmgrid.normalize_longitude(centroid.x)


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
        for lon in range(math.floor(lon0), math.ceil(lon1)):
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
        if regional is not None and in_regional[i]:
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
