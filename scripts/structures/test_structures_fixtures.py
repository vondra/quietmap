"""Small z30 Arrow fixtures shared by the dev1-derived structures regressions."""

import importlib.util
import json
from pathlib import Path

import pyarrow as pa
import pyarrow.ipc as ipc
import shapely
import numpy as np

SPEC = importlib.util.spec_from_file_location(
    "build_structures", Path(__file__).with_name("build-structures.py"))
assert SPEC is not None and SPEC.loader is not None
BUILDER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUILDER)
import structure_inputs as SOURCES
import structure_contract as CONTRACT
GRID = BUILDER.qmgrid
SQUARE = "z9/276/174"


def grid_polygon(geometry):
    return GRID.encode_grid_poly([GRID.lonlat_to_grid(lon, lat)
                                  for lon, lat in geometry.exterior.coords])


def screening_polygons(geometry):
    parts = geometry.geoms if geometry.geom_type == "MultiPolygon" else [geometry]
    return GRID.encode_grid_polygons([
        [[GRID.lonlat_to_grid(lon, lat) for lon, lat in ring.coords]
         for ring in [part.exterior, *part.interiors]] for part in parts])


def buildings_arrow(path, rows):
    schema = pa.schema([
        ("osm_id", pa.int64()), ("centroid_gx", pa.int32()),
        ("centroid_gy", pa.int32()), ("building_type", pa.uint8()),
        ("building_use", pa.uint8()), ("height", pa.float32()),
        ("floors", pa.uint8()), ("name", pa.utf8()),
        ("addr_street", pa.utf8()), ("addr_housenumber", pa.utf8()),
        ("geom", pa.binary()), ("area_m2", pa.float32()),
        ("opening_hours_frac", pa.uint8()), ("source_id", pa.uint16()),
    ], metadata={b"buildings_contract": b"buildings_v3", b"grid": b"z30"})
    with ipc.new_file(path, schema) as writer:
        writer.write_table(pa.table({name: [row[name] for row in rows]
                                     for name in schema.names}, schema=schema))


def barriers_arrow(path, rows):
    converted = []
    for row in rows:
        values = {name: row[name] for name in ("osm_id", "segment_idx", "height", "height_tier")}
        for end in ("start", "end"):
            gx, gy = GRID.lonlat_to_grid(row[end + "_lon"], row[end + "_lat"])
            values[end + "_gx"], values[end + "_gy"] = gx, gy
        converted.append(values)
    schema = pa.schema([
        ("osm_id", pa.int64()), ("segment_idx", pa.int16()),
        ("start_gx", pa.int32()), ("start_gy", pa.int32()),
        ("end_gx", pa.int32()), ("end_gy", pa.int32()),
        ("height", pa.float32()),
        ("height_tier", pa.uint8()),
    ], metadata={b"grid": b"z30"})
    with ipc.new_file(path, schema) as writer:
        writer.write_table(pa.table({name: [row[name] for row in converted]
                                     for name in schema.names}, schema=schema))


class FakeGlobalPrior:
    def __init__(self, mtime=0.0):
        self.input_identity = ("fake-ghsl", mtime)

    def sample(self, _lon, _lat):
        return 12.5


OSM_POLY = shapely.box(14.17000, 49.78000, 14.17020, 49.78016)
OVT_TWIN = shapely.box(14.170004, 49.780003, 14.170204, 49.780163)
OSM_SHED = shapely.box(14.17500, 49.78500, 14.17504, 49.78504)
OSM_HALL = shapely.box(14.17600, 49.78600, 14.17640, 49.78620)  # ~1500 m2 at this lat... set area explicitly
OVT_LONELY = shapely.box(14.17800, 49.78700, 14.17810, 49.78710)

# A contested first choice: a warehouse outline with a separately mapped annex
# inside it (overlapping OSM polygons are ordinary — building plus building
# part), and two Overture footprints that both rank the annex first.
OSM_WAREHOUSE = shapely.box(14.17100, 49.78100, 14.17200, 49.78180)
OSM_ANNEX = shapely.box(14.17130, 49.78120, 14.17150, 49.78140)
OVT_ANNEX_TWIN = shapely.box(14.171302, 49.781202, 14.171502, 49.781402)  # IoU 0.961
OVT_ANNEX_LOOSE = shapely.box(14.171260, 49.781160, 14.171460, 49.781360)  # IoU 0.471


def osm_row(index, polygon, area, height=None, floors=0, use=0, btype=11):
    centroid = polygon.centroid if polygon is not None else shapely.Point(14.174, 49.784)
    gx, gy = GRID.lonlat_to_grid(centroid.x, centroid.y)
    return {
        "osm_id": 1000 + index, "centroid_gx": gx, "centroid_gy": gy,
        "building_type": btype, "building_use": use, "height": height,
        "floors": floors, "name": None, "addr_street": None,
        "addr_housenumber": None,
        "geom": grid_polygon(polygon) if polygon is not None else None,
        "area_m2": area, "opening_hours_frac": 0, "source_id": 0,
    }


def ovt_row(polygon, h=8.0, tier=2, envelope=1):
    centroid = polygon.centroid
    return {"wkb": shapely.to_wkb(polygon), "height_m": h, "tier": tier,
            "clat": centroid.y, "clon": centroid.x, "envelope": envelope}


def write_prepared_roundtrip(root):
    root = Path(root)
    square = root / SQUARE
    square.mkdir(parents=True)
    buildings_arrow(square / "buildings.arrow",
                    [osm_row(0, OSM_POLY, 32.0, height=4.5)])
    barriers_arrow(square / "barriers.arrow", [{
        "osm_id": 77, "segment_idx": 0, "start_lat": 49.78, "start_lon": 14.17,
        "end_lat": 49.7801, "end_lon": 14.1701, "height": 2.5, "height_tier": 0}])
    BUILDER.build_square(SQUARE, root, [ovt_row(OVT_LONELY)], 0,
                         FakeGlobalPrior(), None)


# Actual Overture N49E014 source polygons; preserve every source coordinate.
# b0fdfb07-6472-49fe-8b8b-362a58b3be2e; original WKB SHA-256 7b32e6d19ede281f73bc1ef5d819e9a675bc2a010fa177a8332195d40afafe7d.
REAL_COURTYARD = [[[[13.9999596, 49.0117434], [13.9999549, 49.0117387], [14.0000592, 49.0116739], [14.0001494, 49.0116271], [14.0002239, 49.0117047], [14.0000196, 49.0118046], [14.0000149, 49.0117999], [13.9999596, 49.0117434]], [[14.0001101, 49.0117147], [14.0000713, 49.011678], [14.0000494, 49.0116892], [14.0000864, 49.0117252], [14.0001101, 49.0117147]]]]
# e0ee07a8-4419-4365-9ba3-30781215c82b; original WKB SHA-256 05e5d3948e874ae75a24bc1d207779f7701c796d2bbd3e1c22d4a1a2b51a6fd4.
REAL_MULTIPART = [[[[14.0099528, 49.1865705], [14.0099541, 49.1865723], [14.009978, 49.1865667], [14.0100204, 49.1866048], [14.0099914, 49.1866166], [14.00997, 49.1865929], [14.0099142, 49.1866064], [14.009903, 49.1865868], [14.0099528, 49.1865705]]], [[[14.0099751, 49.1866713], [14.0099555, 49.1866325], [14.0099867, 49.186624], [14.0100085, 49.1866632], [14.0099751, 49.1866713]]], [[[14.0100163, 49.1866449], [14.0100573, 49.1866304], [14.0100733, 49.1866512], [14.0100338, 49.1866671], [14.0100163, 49.1866449]]]]


def write_topology_roundtrip(root):
    root = Path(root)
    cases = []
    for name, parts in [("courtyard", REAL_COURTYARD), ("multipart", REAL_MULTIPART)]:
        original = shapely.MultiPolygon([shapely.Polygon(rings[0], rings[1:]) for rings in parts])
        for seam in [False, True]:
            reference = original.centroid.x
            placed = shapely.transform(original, lambda xy: np.column_stack((
                GRID.normalize_longitude(xy[:, 0] - reference + 180.0) if seam else xy[:, 0], xy[:, 1])))
            case_root = root / f"{name}-{'seam' if seam else 'original'}"
            row = ovt_row(placed, h=12.0, tier=0)
            row["clat"], row["clon"] = SOURCES.footprint_centroid(placed)
            square = GRID.square_name(*GRID.square_of(row["clat"], row["clon"]))
            (case_root / square).mkdir(parents=True)
            # Match one original OSM exterior without changing its emission ring.
            osm_geometry = shapely.Polygon(parts[0][0])
            osm = osm_row(0, osm_geometry, 3000.0, height=4.5)
            if seam:
                osm_geometry = shapely.transform(osm_geometry, lambda xy: np.column_stack((
                    GRID.normalize_longitude(xy[:, 0] - reference + 180.0), xy[:, 1])))
                osm["geom"] = grid_polygon(osm_geometry)
                lat, lon = SOURCES.footprint_centroid(osm_geometry)
                osm["centroid_gx"], osm["centroid_gy"] = GRID.lonlat_to_grid(lon, lat)
            buildings_arrow(case_root / square / "buildings.arrow", [osm])
            BUILDER.build_square(square, case_root, [row], [], FakeGlobalPrior(), None)
            points = []
            for part in original.geoms:
                points.append([part.representative_point().y, part.representative_point().x, True])
                points.extend([shapely.Polygon(hole).representative_point().y,
                               shapely.Polygon(hole).representative_point().x, False]
                              for hole in part.interiors)
            gap = original.convex_hull.difference(original)
            if not gap.is_empty:
                point = gap.representative_point()
                points.append([point.y, point.x, False])
            points.append([original.bounds[3] + 0.0001, original.centroid.x, False])
            if seam:
                points = [[lat, GRID.normalize_longitude(lon - reference + 180.0), inside]
                          for lat, lon, inside in points]
            cases.append({"root": str(case_root), "square": square,
                          "lat": row["clat"], "lon": row["clon"], "points": points,
                          "parts": len(parts), "rings": sum(map(len, parts)),
                          "emission_geom": osm["geom"].hex()})
    (root / "topology.json").write_text(json.dumps(cases))
    return cases
