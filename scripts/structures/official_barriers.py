"""Official noise-barrier inventory: cache reader, OSM replacement, hop rows."""

import math

import pyarrow as pa
import pyarrow.parquet as pq
import shapely
from shapely import STRtree

import qmgrid
from structure_inputs import (
    METRES_PER_DEGREE, footprint_centroid, footprint_in_longitude_frame,
)
from structure_inventory import official_tile_sources

CONTRACT_KEY = "official_barriers_contract"
CONTRACT_VERSION = "official_barriers_v1"

KIND_WALL = 0
KIND_BERM = 1
KIND_COMBINED = 2
# A berm is terrain, not a thin wall: its screening waits for the terrain step.
SCREENED_KINDS = frozenset({KIND_WALL, KIND_COMBINED})

SCHEMA = pa.schema([
    pa.field("geometry", pa.binary(), nullable=False),  # WKB LineString lon/lat
    pa.field("height_m", pa.float32(), nullable=False),
    pa.field("measured", pa.bool_(), nullable=False),  # else the inventory median
    pa.field("kind", pa.uint8(), nullable=False),      # wall 0, berm 1, combined 2
    pa.field("source", pa.utf8(), nullable=False),     # e.g. NL-RWS-GWV-2024
    pa.field("as_of", pa.utf8(), nullable=False),      # inventory vintage YYYY-MM-DD
])

# An OSM micro-segment on the same wall as an official line is the same wall
# twice: survey-vs-trace offsets run 1-3 m, while distinct parallel noise walls
# stand on opposite carriageway sides, 15 m or more apart.
REPLACE_DISTANCE_M = 5.0
# OSM linear ways chord at 250 m (engine/osm-extract/src/pass2.rs); official
# hops never run longer, and keep their surveyed intermediate vertices.
HOP_CAP_M = 250.0


def read_official_parquet(cache_dir, square):
    """The square's official barrier rows from the touched 1-degree tiles,
    assigned by line centroid like Overture footprints. Returns (rows, files)."""
    rows, inputs = [], []
    for _lat, _lon, src in official_tile_sources(cache_dir, square):
        inputs.append(src)
        table = pq.read_table(src, columns=[name for name in SCHEMA.names])
        contract = (table.schema.metadata or {}).get(CONTRACT_KEY.encode())
        if contract != CONTRACT_VERSION.encode():
            raise SystemExit(f"{src}: {CONTRACT_KEY} mismatch "
                             f"(expected {CONTRACT_VERSION}, got {contract!r})")
        for value in table.to_pylist():
            geom = shapely.from_wkb(value["geometry"])
            if geom.is_empty:
                continue
            clat, clon = footprint_centroid(geom)
            if qmgrid.square_of(clat, clon) != square:
                continue
            rows.append({"geom": geom, "clat": clat, "clon": clon,
                         "height_m": value["height_m"], "measured": value["measured"],
                         "kind": value["kind"], "source": value["source"],
                         "as_of": value["as_of"]})
    return rows, inputs


def segment_length_m(lon0, lat0, lon1, lat1):
    dx = (lon1 - lon0) * METRES_PER_DEGREE * math.cos(math.radians((lat0 + lat1) / 2))
    dy = (lat1 - lat0) * METRES_PER_DEGREE
    return math.hypot(dx, dy)


def split_hops(coords):
    """Vertex hops of a lon/lat line, chopped at the OSM micro-segment cap.
    Hops interpolate in one short-arc frame, so a dateline crossing chops the
    short way, not across the map."""
    hops = []
    for (lon0, lat0), (lon1, lat1) in zip(coords, coords[1:]):
        span = qmgrid.wrapped_longitude_delta(lon0, lon1)
        length = segment_length_m(lon0, lat0, lon0 + span, lat1)
        if length <= HOP_CAP_M:
            hops.append(((lon0, lat0), (lon1, lat1), length))
            continue
        count = math.ceil(length / HOP_CAP_M)
        for i in range(count):
            a, b = i / count, (i + 1) / count
            hops.append(((qmgrid.normalize_longitude(lon0 + span * a), lat0 + (lat1 - lat0) * a),
                         (qmgrid.normalize_longitude(lon0 + span * b), lat0 + (lat1 - lat0) * b),
                         length / count))
    return hops


def replacement_tree(official_rows):
    """STRtree over screened official lines in one shared longitude frame."""
    lines = [row["geom"] for row in official_rows if row["kind"] in SCREENED_KINDS]
    if not lines:
        return None, [], 0.0
    reference = float(shapely.get_coordinates(lines[0])[0][0])
    framed = [footprint_in_longitude_frame(line, reference) for line in lines]
    return STRtree(framed), framed, reference


def osm_segment_is_replaced(start_lon, start_lat, end_lon, end_lat, tree, framed, reference):
    """An OSM micro-segment midpoint within the replace distance of an official
    screened line is the same wall and the OSM copy goes."""
    mid_lon = reference + (qmgrid.wrapped_longitude_delta(reference, start_lon)
                           + qmgrid.wrapped_longitude_delta(reference, end_lon)) / 2
    mid_lat = (start_lat + end_lat) / 2
    reach = REPLACE_DISTANCE_M / METRES_PER_DEGREE
    reach_lon = reach / math.cos(math.radians(max(min(mid_lat, 89.9), -89.9)))
    box = shapely.box(mid_lon - reach_lon - 1e-9, mid_lat - reach - 1e-9,
                      mid_lon + reach_lon + 1e-9, mid_lat + reach + 1e-9)
    for k in tree.query(box):
        nearest = shapely.shortest_line(shapely.Point(mid_lon, mid_lat), framed[k])
        (x0, y0), (x1, y1) = shapely.get_coordinates(nearest)
        if segment_length_m(x0, y0, x1, y1) <= REPLACE_DISTANCE_M:
            return True
    return False
