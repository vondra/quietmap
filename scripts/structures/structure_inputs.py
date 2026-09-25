"""Raster height sources and canonical Overture footprint ingestion."""

import math

import numpy as np
import pyarrow as pa
import pyarrow.parquet as pq
import rasterio
import shapely
import shapely.ops
from pyproj import Transformer

import qmgrid
from structure_inventory import overture_sources

MEASURED_MIN_M = 2.0      # zonal pixels below this are "not a building surface here"
COVERAGE_MIN_FRAC = 0.30  # measured pixels must cover this share of the footprint
COVERAGE_MIN_PX = 3

ENVELOPE_OUTDOOR = 0
ENVELOPE_DEFAULT = 5
# OSM envelope-use codes: residential, commercial, industrial, explicit open carport or roof.
BUILDING_USE_OPEN_ROOF = 3
ENVELOPE_FROM_BUILDING_USE = {0: 1, 1: 2, 2: 3, BUILDING_USE_OPEN_ROOF: ENVELOPE_OUTDOOR}


class RegionalHeights:
    """Regional relative-height raster. Windowed reads: the IPR mosaic is 4.2 GiB."""

    def __init__(self, path):
        self.ds = rasterio.open(path)
        self.gt = self.ds.transform
        self.w, self.h = self.ds.width, self.ds.height
        self.tr = Transformer.from_crs("EPSG:4326", self.ds.crs, always_xy=True)
        self.input_files = sorted(self.ds.files)
        self.nodata = self.ds.nodata

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
        window = self.ds.read(1, window=((r0, r1), (c0, c1))).astype(np.float32, copy=False)
        if self.nodata is not None:
            window = np.where(window == self.nodata, np.nan, window)
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
# A roof on posts has no walls to screen with; a greenhouse, a grandstand or a garage has.
OPEN_ROOF_CLASSES = {"carport", "roof"}
OUTDOOR_CLASSES = OPEN_ROOF_CLASSES | {
    "greenhouse", "glasshouse", "bridge_structure", "grandstand",
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


def envelope_class(building_class, subtype):
    """Overture above-ground (class, subtype) -> envelope class 0..5."""
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


def footprint_in_longitude_frame(geom, reference):
    """Move a working GEOS geometry to one shared short-arc longitude frame."""
    minimum_lon, _, maximum_lon, _ = geom.bounds
    if minimum_lon >= reference - 180.0 and maximum_lon < reference + 180.0:
        return geom

    def shift(xy):
        delta = qmgrid.wrapped_longitude_delta(reference, xy[:, 0])
        turns = np.rint((xy[:, 0] - reference - delta) / 360.0)
        # Whole-turn shifts preserve ordinary vertex coordinates exactly.
        return np.column_stack((xy[:, 0] - 360.0 * turns, xy[:, 1]))

    return shapely.transform(geom, shift)


def footprint_centroid(geom):
    """Canonical (lat, lon) centroid using the same short arc as matching."""
    minimum_lon, _, maximum_lon, _ = geom.bounds
    if maximum_lon - minimum_lon <= 180.0:
        centroid = geom.centroid
        return centroid.y, centroid.x
    reference = float(shapely.get_coordinates(geom)[0][0])
    centroid = footprint_in_longitude_frame(geom, reference).centroid
    return centroid.y, qmgrid.normalize_longitude(centroid.x)


def grid_ring_to_shapely(ring):
    """Snapped grid ring -> shapely Polygon in lon/lat (matching geometry)."""
    return shapely.Polygon(qmgrid.ring_to_lonlat(ring))


# The spherical degree the typology's reference areas were measured with (pilot 2026-09-24).
METRES_PER_DEGREE = 111_320.0


def footprint_area_m2(geom, lat):
    """Plan area in square metres on the local equirectangular frame, holes excluded."""
    local = footprint_in_longitude_frame(geom, float(shapely.get_coordinates(geom)[0][0]))
    return local.area * METRES_PER_DEGREE ** 2 * math.cos(math.radians(lat))


def read_overture_parquet(parquet_dir, square):
    """The square's Overture rows from the one-degree parquets: every 1-degree
    tile the square's span touches, rows kept by the half-open tile-ownership
    rule, then assigned to this square by GEOS centroid. A z9 square never
    straddles the antimeridian (spans slice [-180, 180)), so no unwrapping.

    Returns (rows, every contributing parquet file)."""
    rows = []
    inputs = []
    for lat, lon, src in overture_sources(parquet_dir, square):
        inputs.append(src)
        pf = pq.ParquetFile(src)
        have = set(pf.schema_arrow.names)
        cols = [c for c in ("geometry", "height", "num_floors", "class",
                            "subtype", "is_underground") if c in have]
        for batch in pf.iter_batches(columns=cols):
            table = pa.Table.from_batches([batch])
            if "is_underground" in have:
                underground = np.asarray(table.column("is_underground").to_pylist(), dtype=bool)
                table = table.filter(pa.array(~underground))
            geoms = shapely.from_wkb(table.column("geometry").to_numpy())
            polygon = np.isin(shapely.get_type_id(geoms), (3, 6)) & ~shapely.is_empty(geoms)
            indices = np.flatnonzero(polygon)
            footprints = geoms[indices]
            centroids = shapely.centroid(footprints)
            clons, clats = shapely.get_x(centroids), shapely.get_y(centroids)
            bounds = shapely.bounds(footprints)
            # Dateline polygons use the same short-arc centroid as scalar matching.
            for i in np.flatnonzero(bounds[:, 2] - bounds[:, 0] > 180.0):
                clats[i], clons[i] = footprint_centroid(footprints[i])
            owned = (np.isfinite(clats) & np.isfinite(clons)
                     & (clats >= lat) & (clats < lat + 1)
                     & (clons >= lon) & (clons < lon + 1))
            # Keep the canonical scalar grid assignment, including polar and edge rounding.
            selected = [i for i in np.flatnonzero(owned)
                        if qmgrid.square_of(float(clats[i]), float(clons[i])) == square]
            values = table.take(pa.array(indices[selected], type=pa.int64())).to_pylist()
            for i, value in zip(selected, values):
                floors = value.get("num_floors")
                rows.append({"wkb": bytes(value["geometry"]), "overture_height": value.get("height"),
                             # Above uint8 is a tagging error, not a storey count.
                             "overture_floors": floors if floors and 0 < floors <= 255 else 0,
                             "open_roof": value.get("class") in OPEN_ROOF_CLASSES,
                             "clat": float(clats[i]), "clon": float(clons[i]),
                             "envelope": envelope_class(value.get("class"), value.get("subtype"))})
    return rows, inputs

def sample_regional_heights(rows, regional, stats):
    """Fill `regional_m` (survey zonal mean or None) for every row with a footprint inside the
    regional raster. Anywhere else the ladder falls through to mapped floors, Overture, or
    the area typology. Rows are dicts keyed (clat, clon, geom)."""
    n = len(rows)
    for row in rows:
        row["regional_m"] = None
    if n == 0 or regional is None:
        return
    rx, ry = regional.tr.transform([r["clon"] for r in rows], [r["clat"] for r in rows])
    for i, row in enumerate(rows):
        if row["geom"] is None or not regional.covers(rx[i], ry[i]):
            continue
        row["regional_m"] = regional.zonal_measured_mean(row["geom"])
        stats["abstain" if row["regional_m"] is None else "regional"] += 1
