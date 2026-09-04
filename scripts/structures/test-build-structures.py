#!/usr/bin/env python3
"""Regression for build-structures.py — the per-cell merge rule.

Locks the bug classes the world migration cannot rediscover cell by cell:
the match (centroid-in / IoU>=0.5, one-to-one over the complete qualifying pair
set), the ladder provenance per row kind (Overture side wins on matched rows,
OSM tags ladder OSM-only rows), the sparse emission-polygon rule (area >
2000 m2), the emission view's equality with buildings.arrow, the wall row shape
(LineString WKB, midpoint centroid), the idempotent skip over every input, the
empty-cell table, and antimeridian tile discovery and centroid ownership.
"""

import importlib.util
import math
import os
import shutil
import tempfile
import unittest
from pathlib import Path

import h3
import pyarrow as pa
import pyarrow.ipc as ipc
import pyarrow.parquet as pq
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
    """Every centroid samples as 12.5 m of ANBH (tier-2 rows upgrade to 4).
    `mtime` is the ladder raster's freshness stamp the build folds in."""

    def __init__(self, mtime=0.0):
        self.mtime = mtime

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

# A contested first choice: a warehouse outline with a separately mapped annex
# inside it (overlapping OSM polygons are ordinary — building plus building
# part), and two Overture footprints that both rank the annex first.
OSM_WAREHOUSE = shapely.box(14.17100, 49.78100, 14.17200, 49.78180)
OSM_ANNEX = shapely.box(14.17130, 49.78120, 14.17150, 49.78140)
OVT_ANNEX_TWIN = shapely.box(14.171302, 49.781202, 14.171502, 49.781402)  # IoU 0.961
OVT_ANNEX_LOOSE = shapely.box(14.171260, 49.781160, 14.171460, 49.781360)  # IoU 0.471

# The prepared R4 cell whose boundary crosses the antimeridian by the widest
# margin (measured 2026-09-04 over the 121,790 prepared cells: 65 straddle it).
DATELINE_CELL = "84045bbffffffff"


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
        # The OSM extracts are a SOURCE tree beside the prepared cell, never in it,
        # and EVERY cell carries both tables — 0-row where nothing stands. Tests
        # with rows overwrite them; an absent one is a broken tree, not an empty cell.
        self.osm = self.root / "osm-extract"
        (self.osm / CELL).mkdir(parents=True)
        buildings_arrow(self.osm / CELL / "buildings.arrow", [])
        barriers_arrow(self.osm / CELL / "barriers.arrow", [])
        self.staging = self.root / "staging"

    def tearDown(self):
        shutil.rmtree(self.root, ignore_errors=True)

    def build(self, ovt_rows, validate=True):
        if ovt_rows is not None:
            shard(self.staging / CELL / "obstacles-N49E014.arrow", ovt_rows)
        with_env = BUILDER  # module alias for readability
        census = with_env.build_cell(
            CELL, str(self.h3r4), str(self.osm), ovt_rows, None, FakeGlobalPrior(),
            None, validate,
        )
        table = ipc.open_file(str(self.h3r4 / CELL / "structures.arrow")).read_all()
        return census, table

    def test_matched_row_keeps_overture_geometry_and_osm_attributes(self):
        buildings_arrow(self.osm / CELL / "buildings.arrow",
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

    def test_a_contested_first_choice_falls_through_to_the_next_twin(self):
        """Both Overture footprints rank the annex first; the loser must take the
        warehouse it also qualifies against instead of becoming Overture-only.
        Keeping each row's local best alone dropped it, and the annex's twin then
        screened twice — once as the OSM warehouse outline, once as the Overture
        footprint standing in the same place."""
        buildings_arrow(self.osm / CELL / "buildings.arrow", [
            osm_row(0, OSM_WAREHOUSE, 6392.0),
            osm_row(1, OSM_ANNEX, 320.0),
        ])
        census, t = self.build([ovt_row(OVT_ANNEX_TWIN), ovt_row(OVT_ANNEX_LOOSE)])
        self.assertEqual((census["both"], census["osm_only"], census["overture_only"]),
                         (2, 0, 0))
        self.assertEqual(t.num_rows, 2)
        # OSM file order: the warehouse takes the loose footprint (its only
        # remaining qualifying pair), the annex keeps its near twin.
        self.assertEqual(t.column("osm_id").to_pylist(), [1000, 1001])
        self.assertEqual(t.column("geometry_wkb").to_pylist(),
                         [wkb_of(OVT_ANNEX_LOOSE), wkb_of(OVT_ANNEX_TWIN)])
        # Screening moved to the Overture polygons; emission keeps the OSM one
        # wherever it can read it (the warehouse is over the 2000 m2 threshold).
        self.assertEqual(t.column("emission_polygon_wkb").to_pylist(),
                         [wkb_of(OSM_WAREHOUSE), None])

    def test_big_matched_row_carries_the_osm_emission_polygon(self):
        buildings_arrow(self.osm / CELL / "buildings.arrow",
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
        buildings_arrow(self.osm / CELL / "buildings.arrow",
                        [osm_row(0, OSM_HALL, 5000.0)])
        census, t = self.build([ovt_row(hall_twin, h=8.0, tier=2)])
        self.assertEqual(census["both"], 1)
        row = {name: t.column(name)[0].as_py() for name in t.column_names}
        self.assertEqual(row["geometry_wkb"], wkb_of(hall_twin))
        self.assertEqual(row["emission_polygon_wkb"], wkb_of(OSM_HALL))

    def test_osm_only_row_ladders_from_osm_tags(self):
        buildings_arrow(self.osm / CELL / "buildings.arrow", [
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
        buildings_arrow(self.osm / CELL / "buildings.arrow",
                        [osm_row(0, OSM_POLY, 32.0), osm_row(1, OSM_SHED, 16.0)])
        barriers_arrow(self.osm / CELL / "barriers.arrow", [
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
        buildings_arrow(self.osm / CELL / "buildings.arrow", [osm_row(0, OSM_POLY, 32.0)])
        census, _ = self.build([ovt_row(OVT_LONELY)])
        self.assertIsNotNone(census)
        again = BUILDER.build_cell(CELL, str(self.h3r4), str(self.osm),
                                   BUILDER.read_overture_shards(str(self.staging / CELL))[0],
                                   BUILDER.read_overture_shards(str(self.staging / CELL))[1],
                                   FakeGlobalPrior(), None, False)
        self.assertIsNone(again)  # fresh: nothing rebuilt
        # A newer height raster is an input like any other: the ladder feeds
        # every row, so a re-sampled raster that never invalidates would serve a
        # stale table for ever (the parquet mode has no per-cell input at all).
        newer = os.path.getmtime(self.h3r4 / CELL / "structures.arrow") + 5
        rebuilt_for_the_raster = BUILDER.build_cell(
            CELL, str(self.h3r4), str(self.osm),
            *BUILDER.read_overture_shards(str(self.staging / CELL)),
            FakeGlobalPrior(mtime=newer), None, False)
        self.assertIsNotNone(rebuilt_for_the_raster)
        # Touching a per-cell input rebuilds too.
        os.utime(self.osm / CELL / "buildings.arrow",
                 (os.path.getmtime(self.h3r4 / CELL / "structures.arrow") + 5,) * 2)
        refreshed = BUILDER.build_cell(
            CELL, str(self.h3r4), str(self.osm),
            *BUILDER.read_overture_shards(str(self.staging / CELL)),
            FakeGlobalPrior(), None, False)
        self.assertIsNotNone(refreshed)

    def test_a_stale_copy_in_the_prepared_cell_is_ignored(self):
        """The OSM pair used to live in the prepared cell. A leftover there must
        never reach the merge again — the builder reads --osm-dir and nothing
        else, so an un-migrated cell cannot quietly resurrect its old buildings."""
        buildings_arrow(self.osm / CELL / "buildings.arrow",
                        [osm_row(0, OSM_POLY, 32.0)])
        decoy = osm_row(500, OSM_SHED, 16.0)   # osm_id 1500, a different building
        buildings_arrow(self.h3r4 / CELL / "buildings.arrow", [decoy])
        _, t = self.build([ovt_row(OVT_TWIN)])
        self.assertEqual(t.column("osm_id").to_pylist(), [1000])

    def test_an_absent_per_cell_table_is_an_error_not_an_empty_cell(self):
        """Every prepared cell carries both tables, 0-row where nothing stands, so
        an absent one is a broken tree. Read as "no buildings here" it would write
        a valid-looking Overture-only table and drop the cell's emission stock.

        Both entry paths: the fresh build (which loads the tables) and the REBUILD
        path, whose freshness probe used to stat them first and die with a bare
        FileNotFoundError naming neither the cell nor what to do about it."""
        for name in ("buildings.arrow", "barriers.arrow"):
            path = self.osm / CELL / name
            kept = path.read_bytes()

            path.unlink()
            with self.assertRaises(SystemExit) as fresh:
                self.build([ovt_row(OVT_LONELY)], validate=False)
            self.assertIn(CELL, str(fresh.exception))
            self.assertIn(name, str(fresh.exception))

            # Now with a table already on disk, so the mtime probe runs first.
            path.write_bytes(kept)
            self.build([ovt_row(OVT_LONELY)], validate=False)
            self.assertTrue((self.h3r4 / CELL / "structures.arrow").exists())
            path.unlink()
            with self.assertRaises(SystemExit) as rebuild:
                self.build([ovt_row(OVT_LONELY)], validate=False)
            self.assertIn(CELL, str(rebuild.exception))
            self.assertIn(name, str(rebuild.exception))
            path.write_bytes(kept)

    def test_a_missing_or_bare_osm_tree_is_refused(self):
        """The mistake a direct builder call makes: a typo or an unmounted disk.
        Every cell would otherwise fail one by one on a per-file symptom."""
        with self.assertRaises(SystemExit):
            BUILDER.require_osm_tree(str(self.root / "not-here"))
        bare = self.root / "bare"
        (bare / "not-a-cell").mkdir(parents=True)
        with self.assertRaises(SystemExit):
            BUILDER.require_osm_tree(str(bare))
        BUILDER.require_osm_tree(str(self.osm))  # the real tree passes

    def test_stale_buildings_contract_is_rejected(self):
        path = self.osm / CELL / "buildings.arrow"
        buildings_arrow(path, [osm_row(0, OSM_POLY, 32.0)])
        # Re-stamp with an outdated contract: the merge must refuse to certify it.
        t = ipc.open_file(str(path)).read_all()
        schema = t.schema.with_metadata({b"buildings_contract": b"buildings_v1"})
        with ipc.new_file(str(path), schema) as w:
            w.write_table(t)
        with self.assertRaises(SystemExit):
            self.build([ovt_row(OVT_LONELY)])


def overture_parquet(path, geoms):
    """One 1-degree tile of the download cache: the columns the builder reads."""
    n = len(geoms)
    table = pa.table({
        "geometry": pa.array([wkb_of(g) for g in geoms], pa.binary()),
        "height": pa.array([None] * n, pa.float64()),
        "num_floors": pa.array([None] * n, pa.int32()),
        "class": pa.array(["house"] * n, pa.string()),
        "subtype": pa.array(["residential"] * n, pa.string()),
        "is_underground": pa.array([False] * n, pa.bool_()),
    })
    path.parent.mkdir(parents=True, exist_ok=True)
    pq.write_table(table, str(path))


class AntimeridianTests(unittest.TestCase):
    """Tile discovery and centroid ownership across the dateline. Plain min/max
    longitudes turn one R4 cell into a scan of the whole planet, and a planar
    centroid puts a footprint stored across +/-180 deg near 0 deg — the cell
    that owns it never sees it, and a cell 100 deg away is asked for it."""

    def test_tile_columns_unwrap_around_the_cell_centre(self):
        boundary_lons = [p[1] for p in h3.cell_to_boundary(DATELINE_CELL)]
        # What the plain box says: nearly every longitude on Earth.
        self.assertGreater(max(boundary_lons) - min(boundary_lons), 180.0)
        self.assertEqual(
            list(BUILDER.cell_tile_columns(DATELINE_CELL, boundary_lons)), [179, 180]
        )
        # An ordinary cell keeps the plain range.
        lons = [p[1] for p in h3.cell_to_boundary(CELL)]
        self.assertEqual(
            list(BUILDER.cell_tile_columns(CELL, lons)),
            list(range(math.floor(min(lons)), math.floor(max(lons)) + 1)),
        )

    def test_footprint_centroid_crosses_the_dateline(self):
        crossing = shapely.Polygon([
            (179.99, 49.7800), (-179.996, 49.7800),
            (-179.996, 49.7802), (179.99, 49.7802),
        ])
        lat, lon = BUILDER.footprint_centroid(crossing)
        self.assertAlmostEqual(lon, 179.997, places=9)
        self.assertAlmostEqual(lat, 49.7801, places=9)
        # The planar centroid it replaces sits a third of the way round the world.
        self.assertAlmostEqual(crossing.centroid.x, -0.003, places=9)
        # An ordinary footprint takes the untouched centroid, value for value.
        lat, lon = BUILDER.footprint_centroid(OSM_POLY)
        self.assertEqual((lat, lon), (OSM_POLY.centroid.y, OSM_POLY.centroid.x))

    def test_parquet_mode_reads_a_dateline_cell_once(self):
        root = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, root, True)
        centre_lat = h3.cell_to_latlng(DATELINE_CELL)[0]
        crossing = shapely.Polygon([
            (179.99, centre_lat), (-179.996, centre_lat),
            (-179.996, centre_lat + 0.0002), (179.99, centre_lat + 0.0002),
        ])
        # The row's bbox spans the planet, so the downloader stored it in BOTH
        # tiles the cell touches; the half-open rule must keep it exactly once.
        overture_parquet(root / "N71E179.parquet", [crossing])
        overture_parquet(root / "N71W180.parquet", [crossing])
        rows, mtime = BUILDER.read_overture_parquet(str(root), DATELINE_CELL)
        self.assertEqual(len(rows), 1)
        self.assertAlmostEqual(rows[0]["clon"], 179.997, places=9)
        self.assertEqual(h3.latlng_to_cell(rows[0]["clat"], rows[0]["clon"], 4),
                         DATELINE_CELL)
        self.assertEqual(mtime, max(os.path.getmtime(str(root / name))
                                    for name in ("N71E179.parquet", "N71W180.parquet")))


if __name__ == "__main__":
    unittest.main(verbosity=2)
