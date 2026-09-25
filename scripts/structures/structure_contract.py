"""The structures_v5 Arrow contract, source decoding, and emission preservation proof."""

import pyarrow as pa
import pyarrow.compute as pc
import pyarrow.ipc as ipc
import os
import math

import qmgrid
from structure_inputs import grid_ring_to_shapely

KIND_BUILDING = 0
KIND_BARRIER = 1

CONTRACT_KEY = "structures_contract"
# z30 geometry, Int16 screening metres, the height source and the demand storey count.
CONTRACT_VERSION = "structures_v5"

# Where a row's screening height came from (square_store::structure_contract mirrors these).
HEIGHT_SOURCE_OSM_HEIGHT = 0          # mapped OSM `height` (buildings and walls)
HEIGHT_SOURCE_FLOORS = 1              # OSM, national or Overture floors x storey + roof
HEIGHT_SOURCE_AREA_TYPOLOGY = 2       # footprint-area typology: not per building
HEIGHT_SOURCE_REGIONAL_MEASURED = 3   # regional survey zonal mean (Prague LiDAR)
HEIGHT_SOURCE_GHSL = 4                # retired 2026-09-25: read from older prepared only
HEIGHT_SOURCE_OVERTURE_HEIGHT = 5     # Overture height (OSM-derived or machine-learned)
HEIGHT_SOURCE_OPEN_ROOF = 6           # open roof or carport: footprint stays, screens 0 m
HEIGHT_SOURCE_GROUND_ACTIVITY = 7     # emission-only ground: no screening geometry, 0 m
HEIGHT_SOURCE_WALL_DEFAULT = 8        # unmapped noise wall at its country's mean height

# Where a building row's demand storey count came from.
STOREYS_SOURCE_FLOORS = 0             # OSM, national or Overture floors
STOREYS_SOURCE_LADDER_HEIGHT = 1      # inverse of the floors rung on the screening height
STOREYS_SOURCE_SINGLE_LEVEL = 2       # ground activity or open roof without floors

SCHEMA = pa.schema(
    [
        pa.field("kind", pa.uint8(), nullable=False),
        # Building parts/rings (encode_grid_polygons); barriers keep encode_grid_poly.
        pa.field("geom", pa.binary()),
        pa.field("height_m", pa.int16(), nullable=False),
        pa.field("height_source", pa.uint8(), nullable=False),
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
        # Original OSM ring, including null; centroid overrides retain OSM positions.
        pa.field("emission_geom", pa.binary()),
        pa.field("emission_centroid_gx", pa.int32()),
        pa.field("emission_centroid_gy", pa.int32()),
        # Demand storeys (building rows only): the service tree reads this, never raw floors.
        pa.field("storeys", pa.uint8()),
        pa.field("storeys_source", pa.uint8()),
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
    "area_m2", "opening_hours_frac", "source_id", "area_source",
]

# The columns the emission view is validated against, in buildings.arrow order.
EMISSION_COMPARE = [
    "osm_id", "building_type", "building_use", "height", "floors", "name",
    "addr_street", "addr_housenumber", "area_m2", "opening_hours_frac",
    "source_id",
]


def screening_height_metres(value):
    if not math.isfinite(value) or not 0 <= value <= 32767:
        raise ValueError(f"screening height outside Int16 metres: {value!r}")
    # Match Rust's round for nonnegative physical heights; never truncate.
    return math.floor(value + 0.5)

def require_column(table, path, name, dtype):
    if name not in table.column_names:
        raise SystemExit(f"{path}: missing {name} — re-extract OSM")
    column = table.column(name)
    if column.type != dtype or column.null_count:
        raise SystemExit(f"{path}: {name} must be non-null {dtype} — re-extract OSM")


def require_grid_contract(table, path, coordinates):
    grid_pin = (table.schema.metadata or {}).get(b"grid")
    if grid_pin != b"z30":
        raise SystemExit(f"{path}: grid pin mismatch (expected z30, got {grid_pin!r})")
    for name in coordinates:
        require_column(table, path, name, pa.int32())


def load_osm_buildings(path):
    if not os.path.exists(path):
        return {column: [] for column in [*BUILDINGS_COLUMNS, "shapely"]}
    t = ipc.open_file(path).read_all()
    contract = (t.schema.metadata or {}).get(b"buildings_contract")
    if contract != b"buildings_v5":
        raise SystemExit(
            f"{path}: buildings_contract mismatch (expected buildings_v5, got "
            f"{contract!r}) — re-extract OSM"
        )
    require_grid_contract(t, path, ("centroid_gx", "centroid_gy"))
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
    require_grid_contract(t, path, ("start_gx", "start_gy", "end_gx", "end_gy"))
    require_column(t, path, "height_tier", pa.uint8())
    # The wall default is national: square-country-city bakes each wall's country.
    if (t.schema.metadata or {}).get(b"barriers_contract") != b"country_baked_v1":
        raise SystemExit(f"{path}: walls lack their country — run square-country-city")
    require_column(t, path, "country_iso", pa.uint16())
    cols = {c: t.column(c).to_pylist()
            for c in ("osm_id", "segment_idx", "start_gx", "start_gy",
                      "end_gx", "end_gy", "height", "height_tier", "country_iso")}
    return [dict(zip(cols.keys(), vals)) for vals in zip(*cols.values())]


def wall_grid_poly(s_gx, s_gy, e_gx, e_gy):
    """2-point grid polyline — the wall micro-segment's geometry."""
    return qmgrid.encode_grid_poly([(s_gx, s_gy), (e_gx, e_gy)])


def wall_centroid_grid(s_gx, s_gy, e_gx, e_gy):
    lon0, lat0 = qmgrid.grid_to_lonlat(s_gx, s_gy)
    lon1, lat1 = qmgrid.grid_to_lonlat(e_gx, e_gy)
    return qmgrid.lonlat_to_grid(
        qmgrid.wrapped_longitude_midpoint(lon0, lon1), (lat0 + lat1) / 2.0
    )

def validate_square(name, osm, table):
    """Emission-view proof for one square (raises, never warns): the emission
    view (kind=0, osm_id present, file order) equals buildings.arrow row by row
    on every emission column, original emission_geom and emission centroids."""
    mask = pc.call_function("and", [
        pc.call_function("equal", [table.column("kind"), KIND_BUILDING]),
        pc.call_function("is_valid", [table.column("osm_id")]),
    ])
    view = table.filter(mask)
    n = len(osm["osm_id"])
    if view.num_rows != n:
        raise SystemExit(
            f"{name}: emission view rows {view.num_rows} != buildings.arrow {n}"
        )
    cols = {c: view.column(c).to_pylist() for c in EMISSION_COMPARE}
    egeom = view.column("emission_geom").to_pylist()
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
        if egeom[i] != osm["geom"][i]:
            raise SystemExit(f"{name}: emission row {i} polygon differs")
        if (egx[i] if egx[i] is not None else cgx[i]) != osm["centroid_gx"][i]:
            raise SystemExit(f"{name}: emission row {i} centroid_gx differs")
        if (egy[i] if egy[i] is not None else cgy[i]) != osm["centroid_gy"][i]:
            raise SystemExit(f"{name}: emission row {i} centroid_gy differs")
