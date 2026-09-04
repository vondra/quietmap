#!/usr/bin/env python3
"""Regression for build-structures.py — the per-cell merge rule.

Locks the bug classes the world migration cannot rediscover cell by cell:
the match (centroid-in / IoU>=0.5, one-to-one), the ladder provenance per row
kind (Overture side wins on matched rows, OSM tags ladder OSM-only rows), the
sparse emission-polygon rule (area > 2000 m2), the emission view's equality
with buildings.arrow, the wall row shape (LineString WKB, midpoint centroid),
the idempotent skip, and the empty-cell table.
"""

import importlib.util
import os
import shutil
import tempfile
import unittest
from pathlib import Path

import pyarrow as pa
import pyarrow.ipc as ipc
import shapely
from shapely import wkb as shapely_wkb

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("build_structures", HERE / "build-structures.py")
BUILDER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUILDER)

CELL = "841e309ffffffff"


def wkb_of(geom):
    return shapely.to_wkb(geom, output_dimension=2)


def buildings_arrow(path, rows):
    """rows: list of dicts with the BUILDINGS_COLUMNS keys."""
    cols = {c: [r[c] for r in rows] for c in BUILDER.BUILDINGS_COLUMNS}
    schema = pa.schema(
        [
            ("osm_id", pa.int64()), ("centroid_lat", pa.float64()),
            ("centroid_lon", pa.float64()), ("building_type", pa.uint8()),
            ("building_use", pa.uint8()), ("height", pa.float32()),
            ("floors", pa.uint8()), ("name", pa.utf8()),
            ("addr_street", pa.utf8()), ("addr_housenumber", pa.utf8()),
            ("polygon_wkb", pa.binary()), ("area_m2", pa.float32()),
            ("opening_hours_frac", pa.uint8()), ("source_id", pa.uint16()),
        ],
        metadata={b"buildings_contract": b"buildings_v2"},
    )
    with ipc.new_file(str(path), schema) as w:
        w.write_table(pa.table(cols, schema=schema))


def barriers_arrow(path, rows):
    cols = {c: [r[c] for r in rows]
            for c in ("osm_id", "segment_idx", "start_lat", "start_lon",
                      "end_lat", "end_lon", "height")}
    schema = pa.schema([
        ("osm_id", pa.int64()), ("segment_idx", pa.int16()),
        ("start_lat", pa.float64()), ("start_lon", pa.float64()),
        ("end_lat", pa.float64()), ("end_lon", pa.float64()),
        ("height", pa.float32()),
    ])
    with ipc.new_file(str(path), schema) as w:
        w.write_table(pa.table(cols, schema=schema))


def shard(path, rows):
    cols = {
        "polygon_wkb": pa.array([r["wkb"] for r in rows], pa.binary()),
        "height_m": pa.array([r["height_m"] for r in rows], pa.float32()),
        "centroid_lat": pa.array([r["clat"] for r in rows], pa.float64()),
        "centroid_lon": pa.array([r["clon"] for r in rows], pa.float64()),
        "height_tier": pa.array([r["tier"] for r in rows], pa.uint8()),
        "envelope_class": pa.array([r.get("envelope", 5) for r in rows], pa.uint8()),
    }
    path.parent.mkdir(parents=True, exist_ok=True)
    table = pa.table(cols)
    with ipc.new_file(str(path), table.schema) as w:
        w.write_table(table)


class FakeGlobalPrior:
    """Every centroid samples as 12.5 m of ANBH (tier-2 rows upgrade to 4)."""

    def sample(self, _lon, _lat):
        return 12.5


# A 20 m square at the cell's heart; its twin re-traced ~0.5 m off (IoU > 0.9);
# a small OSM shed far away; a node-only OSM row; a big OSM hall (area > 2000);
# an Overture-only footprint elsewhere.
OSM_POLY = shapely.box(14.17000, 49.78000, 14.17020, 49.78016)
OVT_TWIN = shapely.box(14.170004, 49.780003, 14.170204, 49.780163)
OSM_SHED = shapely.box(14.17500, 49.78500, 14.17504, 49.78504)
OSM_HALL = shapely.box(14.17600, 49.78600, 14.17640, 49.78620)  # ~1500 m2 at this lat... set area explicitly
OVT_LONELY = shapely.box(14.17800, 49.78700, 14.17810, 49.78710)


def osm_row(i, poly, area, height=None, floors=0, use=0, btype=11):
    centroid = poly.centroid if poly is not None else shapely.Point(14.174, 49.784)
    return {
        "osm_id": 1000 + i, "centroid_lat": centroid.y, "centroid_lon": centroid.x,
        "building_type": btype, "building_use": use, "height": height,
        "floors": floors, "name": None, "addr_street": None,
        "addr_housenumber": None, "polygon_wkb": wkb_of(poly) if poly is not None else None,
        "area_m2": area, "opening_hours_frac": 0, "source_id": 0,
    }


def ovt_row(poly, h=8.0, tier=2, envelope=1):
    c = poly.centroid
    return {"wkb": wkb_of(poly), "height_m": h, "tier": tier,
            "clat": c.y, "clon": c.x, "envelope": envelope}


class BuildStructuresTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp())
        self.h3r4 = self.root / "h3r4"
        (self.h3r4 / CELL).mkdir(parents=True)
        self.staging = self.root / "staging"

    def tearDown(self):
        shutil.rmtree(self.root, ignore_errors=True)

    def build(self, ovt_rows, validate=True):
        if ovt_rows is not None:
            shard(self.staging / CELL / "obstacles-N49E014.arrow", ovt_rows)
        with_env = BUILDER  # module alias for readability
        census = with_env.build_cell(
            CELL, str(self.h3r4), ovt_rows, None, FakeGlobalPrior(), None,
            validate, False,
        )
        table = ipc.open_file(str(self.h3r4 / CELL / "structures.arrow")).read_all()
        return census, table

    def test_matched_row_keeps_overture_geometry_and_osm_attributes(self):
        buildings_arrow(self.h3r4 / CELL / "buildings.arrow",
                        [osm_row(0, OSM_POLY, 32.0)])
        census, t = self.build([ovt_row(OVT_TWIN, h=8.0, tier=2)])
        self.assertEqual(census["both"], 1)
        self.assertEqual(t.num_rows, 1)
        row = {name: t.column(name)[0].as_py() for name in t.column_names}
        self.assertEqual(row["kind"], 0)
        self.assertEqual(row["geometry_wkb"], wkb_of(OVT_TWIN))  # screening geometry
        self.assertEqual(row["osm_id"], 1000)
        self.assertEqual(row["building_type"], 11)
        # Matched: Overture ladder height (tier 2 -> GHSL 12.5 -> tier 4), the
        # raw OSM height stays the emission input.
        self.assertEqual(row["height_tier"], 4)
        self.assertAlmostEqual(row["height_m"], 12.5)
        self.assertIsNone(row["height"])
        # The screening centroid is the Overture one; the OSM centroid rides the
        # emission override.
        self.assertAlmostEqual(row["centroid_lat"], OVT_TWIN.centroid.y)
        self.assertAlmostEqual(row["emission_centroid_lat"], OSM_POLY.centroid.y)
        # Small footprint: emission never grids it, so no emission polygon.
        self.assertIsNone(row["emission_polygon_wkb"])

    def test_big_matched_row_carries_the_osm_emission_polygon(self):
        buildings_arrow(self.h3r4 / CELL / "buildings.arrow",
                        [osm_row(0, OSM_HALL, 5000.0)])
        census, t = self.build([ovt_row(OVT_TWIN, h=9.0, tier=1)])
        # The hall and the twin do not overlap: no match, two building rows.
        self.assertEqual(census["both"], 0)
        self.assertEqual(census["overture_only"], 1)
        self.assertEqual(t.num_rows, 2)
        hall = t.slice(0, 1)
        self.assertEqual(hall.column("geometry_wkb")[0].as_py(), wkb_of(OSM_HALL))
        # OSM-only big row: screening polygon IS the OSM polygon, so the
        # emission override stays null.
        self.assertIsNone(hall.column("emission_polygon_wkb")[0].as_py())

    def test_big_matched_row_stores_the_osm_polygon_for_emission(self):
        hall_twin = shapely.box(14.176004, 49.786003, 14.176404, 49.786203)
        buildings_arrow(self.h3r4 / CELL / "buildings.arrow",
                        [osm_row(0, OSM_HALL, 5000.0)])
        census, t = self.build([ovt_row(hall_twin, h=8.0, tier=2)])
        self.assertEqual(census["both"], 1)
        row = {name: t.column(name)[0].as_py() for name in t.column_names}
        self.assertEqual(row["geometry_wkb"], wkb_of(hall_twin))
        self.assertEqual(row["emission_polygon_wkb"], wkb_of(OSM_HALL))

    def test_osm_only_row_ladders_from_osm_tags(self):
        buildings_arrow(self.h3r4 / CELL / "buildings.arrow", [
            osm_row(0, OSM_SHED, 16.0, height=4.5),       # mapped -> tier 0
            osm_row(1, OSM_HALL, 5000.0, floors=5),       # floors -> tier 1, 15 m
            osm_row(2, None, None),                       # node row, default 8 m
        ])
        census, t = self.build([], validate=False)
        self.assertEqual(t.num_rows, 3)
        self.assertEqual(t.column("height_tier").to_pylist(), [0, 1, 2])
        self.assertEqual(t.column("height_m").to_pylist(), [4.5, 15.0, 8.0])
        # The node row has no geometry; the others do.
        self.assertIsNone(t.column("geometry_wkb")[2].as_py())

    def test_row_order_is_osm_then_overture_only_then_walls(self):
        buildings_arrow(self.h3r4 / CELL / "buildings.arrow",
                        [osm_row(0, OSM_POLY, 32.0), osm_row(1, OSM_SHED, 16.0)])
        barriers_arrow(self.h3r4 / CELL / "barriers.arrow", [
            {"osm_id": 55, "segment_idx": 0, "start_lat": 49.78, "start_lon": 14.17,
             "end_lat": 49.7801, "end_lon": 14.1702, "height": 3.0},
            {"osm_id": 55, "segment_idx": 1, "start_lat": 49.7801, "start_lon": 14.1702,
             "end_lat": 49.7802, "end_lon": 14.1703, "height": 4.5},
        ])
        census, t = self.build([ovt_row(OVT_TWIN), ovt_row(OVT_LONELY)])
        self.assertEqual(census["both"], 1)
        self.assertEqual(t.num_rows, 5)
        self.assertEqual(t.column("kind").to_pylist(), [0, 0, 0, 1, 1])
        self.assertEqual(t.column("osm_id").to_pylist(), [1000, 1001, None, 55, 55])
        self.assertEqual(t.column("segment_idx").to_pylist(), [None, None, None, 0, 1])
        # Wall rows: LineString WKB, midpoint centroid, mapped/default tier.
        g = shapely_wkb.loads(t.column("geometry_wkb")[3].as_py())
        self.assertEqual(g.geom_type, "LineString")
        # Tiers: matched row = Overture tier 2 -> GHSL 4; the shed has no tags
        # (8 m -> GHSL 4); the lonely Overture row likewise; walls: default 3.0
        # m -> inferred tier 2, mapped 4.5 m -> tier 0.
        self.assertEqual(t.column("height_tier").to_pylist(), [4, 4, 4, 2, 0])
        self.assertEqual(t.column("height_m").to_pylist()[-2:], [3.0, 4.5])
        meta = t.schema.metadata
        self.assertEqual(meta[b"structures_contract"], b"structures_v1")
        self.assertEqual(meta[b"building_rows"], b"3")
        self.assertEqual(meta[b"barrier_rows"], b"2")

    def test_empty_cell_writes_a_zero_row_table(self):
        census, t = self.build([], validate=False)
        self.assertEqual(t.num_rows, 0)
        self.assertEqual(t.column_names, BUILDER.SCHEMA.names)
        self.assertEqual(t.schema.metadata[b"building_rows"], b"0")

    def test_idempotent_skip_and_input_refresh(self):
        buildings_arrow(self.h3r4 / CELL / "buildings.arrow", [osm_row(0, OSM_POLY, 32.0)])
        census, _ = self.build([ovt_row(OVT_LONELY)])
        self.assertIsNotNone(census)
        again = BUILDER.build_cell(CELL, str(self.h3r4),
                                   BUILDER.read_overture_shards(str(self.staging / CELL))[0],
                                   BUILDER.read_overture_shards(str(self.staging / CELL))[1],
                                   FakeGlobalPrior(), None, False, False)
        self.assertIsNone(again)  # fresh: nothing rebuilt
        # Touching an input rebuilds.
        os.utime(self.h3r4 / CELL / "buildings.arrow",
                 (os.path.getmtime(self.h3r4 / CELL / "structures.arrow") + 5,) * 2)
        refreshed = BUILDER.build_cell(
            CELL, str(self.h3r4),
            *BUILDER.read_overture_shards(str(self.staging / CELL)),
            FakeGlobalPrior(), None, False, False)
        self.assertIsNotNone(refreshed)

    def test_stale_buildings_contract_is_rejected(self):
        path = self.h3r4 / CELL / "buildings.arrow"
        buildings_arrow(path, [osm_row(0, OSM_POLY, 32.0)])
        # Re-stamp with an outdated contract: the merge must refuse to certify it.
        t = ipc.open_file(str(path)).read_all()
        schema = t.schema.with_metadata({b"buildings_contract": b"buildings_v1"})
        with ipc.new_file(str(path), schema) as w:
            w.write_table(t)
        with self.assertRaises(SystemExit):
            self.build([ovt_row(OVT_LONELY)])

    def test_retire_inputs_removes_the_premerge_files(self):
        buildings_arrow(self.h3r4 / CELL / "buildings.arrow", [osm_row(0, OSM_POLY, 32.0)])
        barriers_arrow(self.h3r4 / CELL / "barriers.arrow", [
            {"osm_id": 55, "segment_idx": 0, "start_lat": 49.78, "start_lon": 14.17,
             "end_lat": 49.7801, "end_lon": 14.1702, "height": 3.0},
        ])
        census = BUILDER.build_cell(
            CELL, str(self.h3r4), [], None, FakeGlobalPrior(), None, False, True,
        )
        self.assertFalse((self.h3r4 / CELL / "buildings.arrow").exists())
        self.assertFalse((self.h3r4 / CELL / "barriers.arrow").exists())
        self.assertTrue((self.h3r4 / CELL / "structures.arrow").exists())


if __name__ == "__main__":
    unittest.main(verbosity=2)
