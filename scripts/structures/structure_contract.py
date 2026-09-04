"""The structures_v3 Arrow contract, source decoding, and emission preservation proof."""

import pyarrow as pa
import pyarrow.compute as pc
import pyarrow.ipc as ipc
import os
import math

import qmgrid
from structure_inputs import grid_ring_to_shapely

KIND_BUILDING = 0
KIND_BARRIER = 1
EMISSION_GRID_THRESHOLD_M2 = 2000.0  # normalize/points.rs building grid threshold.

CONTRACT_KEY = "structures_contract"
CONTRACT_VERSION = "structures_v3"  # z30 geometry and Int16 screening metres.

SCHEMA = pa.schema(
    [
        pa.field("kind", pa.uint8(), nullable=False),
        # Snapped grid polygon (qmgrid.encode_grid_poly form), screening stock.
        pa.field("geom", pa.binary()),
        pa.field("height_m", pa.int16(), nullable=False),
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
    if contract != b"buildings_v3":
        raise SystemExit(
            f"{path}: buildings_contract mismatch (expected buildings_v3, got "
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
    cols = {c: t.column(c).to_pylist()
            for c in ("osm_id", "segment_idx", "start_gx", "start_gy",
                      "end_gx", "end_gy", "height", "height_tier")}
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
    on every emission column, with the emission polygon = emission_geom ??
    geom and the emission centroid = emission_centroid_* ?? centroid_*."""
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
