"""Dev1 merge, height, emission, freshness and geographic ownership regressions on z9."""

import os
from pathlib import Path
import tempfile
import unittest

import pyarrow as pa
import pyarrow.ipc as ipc
import pyarrow.parquet as pq
import shapely

from test_structures_fixtures import (
    BUILDER, SOURCES, CONTRACT, GRID, SQUARE, FakeGlobalPrior, buildings_arrow, barriers_arrow,
    grid_polygon, osm_row, ovt_row, OSM_POLY, OVT_TWIN, OSM_SHED, OSM_HALL,
    OVT_LONELY, OSM_WAREHOUSE, OSM_ANNEX, OVT_ANNEX_TWIN, OVT_ANNEX_LOOSE,
)


class BuildStructuresTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.prepared = Path(self.temporary.name)
        (self.prepared / SQUARE).mkdir(parents=True)

    def build(self, rows):
        census = BUILDER.build_square(
            SQUARE, self.prepared, rows, 0, FakeGlobalPrior(), None)
        table = ipc.open_file(self.prepared / SQUARE / "structures.arrow").read_all()
        return census, table

    def test_matched_row_keeps_overture_geometry_and_osm_attributes(self):
        buildings_arrow(self.prepared / SQUARE / "buildings.arrow",
                        [osm_row(0, OSM_POLY, 32.0)])
        census, t = self.build([ovt_row(OVT_TWIN, h=8.0, tier=2)])
        self.assertEqual(census["both"], 1)
        self.assertEqual(t.num_rows, 1)
        row = {name: t.column(name)[0].as_py() for name in t.column_names}
        self.assertEqual(row["kind"], 0)
        self.assertEqual(row["geom"], grid_polygon(OVT_TWIN))  # screening geometry
        self.assertEqual(row["osm_id"], 1000)
        self.assertEqual(row["building_type"], 11)
        # Matched: Overture ladder height (tier 2 -> GHSL 12.5 -> tier 4), the
        # raw OSM height stays the emission input.
        self.assertEqual(row["height_tier"], 4)
        self.assertAlmostEqual(row["height_m"], 12.5)
        self.assertIsNone(row["height"])
        # The screening centroid is the Overture one; the OSM centroid rides the
        # emission override.
        self.assertEqual(row["centroid_gx"], GRID.lonlat_to_grid(OVT_TWIN.centroid.x, OVT_TWIN.centroid.y)[0])
        self.assertEqual(row["emission_centroid_gx"], GRID.lonlat_to_grid(OSM_POLY.centroid.x, OSM_POLY.centroid.y)[0])
        # Small footprint: emission never grids it, so no emission polygon.
        self.assertIsNone(row["emission_geom"])

    def test_a_contested_first_choice_falls_through_to_the_next_twin(self):
        """Both Overture footprints rank the annex first; the loser must take the
        warehouse it also qualifies against instead of becoming Overture-only.
        Keeping each row's local best alone dropped it, and the annex's twin then
        screened twice — once as the OSM warehouse outline, once as the Overture
        footprint standing in the same place."""
        buildings_arrow(self.prepared / SQUARE / "buildings.arrow", [
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
        self.assertEqual(t.column("geom").to_pylist(),
                         [grid_polygon(OVT_ANNEX_LOOSE), grid_polygon(OVT_ANNEX_TWIN)])
        # Screening moved to the Overture polygons; emission keeps the OSM one
        # wherever it can read it (the warehouse is over the 2000 m2 threshold).
        self.assertEqual(t.column("emission_geom").to_pylist(),
                         [grid_polygon(OSM_WAREHOUSE), None])

    def test_big_osm_only_row_keeps_emission_override_null(self):
        buildings_arrow(self.prepared / SQUARE / "buildings.arrow",
                        [osm_row(0, OSM_HALL, 5000.0)])
        census, t = self.build([ovt_row(OVT_TWIN, h=9.0, tier=1)])
        # The hall and the twin do not overlap: no match, two building rows.
        self.assertEqual(census["both"], 0)
        self.assertEqual(census["overture_only"], 1)
        self.assertEqual(t.num_rows, 2)
        hall = t.slice(0, 1)
        self.assertEqual(hall.column("geom")[0].as_py(), grid_polygon(OSM_HALL))
        # OSM-only big row: screening polygon IS the OSM polygon, so the
        # emission override stays null.
        self.assertIsNone(hall.column("emission_geom")[0].as_py())

    def test_big_matched_row_stores_the_osm_polygon_for_emission(self):
        hall_twin = shapely.box(14.176004, 49.786003, 14.176404, 49.786203)
        buildings_arrow(self.prepared / SQUARE / "buildings.arrow",
                        [osm_row(0, OSM_HALL, 5000.0)])
        census, t = self.build([ovt_row(hall_twin, h=8.0, tier=2)])
        self.assertEqual(census["both"], 1)
        row = {name: t.column(name)[0].as_py() for name in t.column_names}
        self.assertEqual(row["geom"], grid_polygon(hall_twin))
        self.assertEqual(row["emission_geom"], grid_polygon(OSM_HALL))

    def test_osm_only_row_ladders_from_osm_tags(self):
        buildings_arrow(self.prepared / SQUARE / "buildings.arrow", [
            osm_row(0, OSM_SHED, 16.0, height=4.5),       # mapped -> tier 0
            osm_row(1, OSM_HALL, 5000.0, floors=5),       # floors -> tier 1, 15 m
            osm_row(2, None, None),                       # node row, default 8 m
        ])
        census, t = self.build([])
        self.assertEqual(t.num_rows, 3)
        self.assertEqual(t.column("height_tier").to_pylist(), [0, 1, 2])
        self.assertEqual(t.column("height_m").to_pylist(), [4.5, 15.0, 8.0])
        # The node row has no geometry; the others do.
        self.assertIsNone(t.column("geom")[2].as_py())

    def test_row_order_is_osm_then_overture_only_then_walls(self):
        buildings_arrow(self.prepared / SQUARE / "buildings.arrow",
                        [osm_row(0, OSM_POLY, 32.0), osm_row(1, OSM_SHED, 16.0)])
        barriers_arrow(self.prepared / SQUARE / "barriers.arrow", [
            {"osm_id": 55, "segment_idx": 0, "start_lat": 49.78, "start_lon": 14.17,
             "end_lat": 49.7801, "end_lon": 14.1702, "height": 3.0, "height_tier": 2},
            {"osm_id": 55, "segment_idx": 1, "start_lat": 49.7801, "start_lon": 14.1702,
             "end_lat": 49.7802, "end_lon": 14.1703, "height": 4.5, "height_tier": 0},
        ])
        census, t = self.build([ovt_row(OVT_TWIN), ovt_row(OVT_LONELY)])
        self.assertEqual(census["both"], 1)
        self.assertEqual(t.num_rows, 5)
        self.assertEqual(t.column("kind").to_pylist(), [0, 0, 0, 1, 1])
        self.assertEqual(t.column("osm_id").to_pylist(), [1000, 1001, None, 55, 55])
        self.assertEqual(t.column("segment_idx").to_pylist(), [None, None, None, 0, 1])
        # Wall rows: two-point grid polyline, midpoint centroid, mapped/default tier.
        ring = GRID.decode_grid_poly(t.column("geom")[3].as_py())
        self.assertEqual(len(ring), 2)
        self.assertEqual(ring[0], GRID.lonlat_to_grid(14.17, 49.78))
        # Unmapped buildings use GHSL; walls retain the extractor's explicit tiers.
        self.assertEqual(t.column("height_tier").to_pylist(), [4, 4, 4, 2, 0])
        self.assertEqual(t.column("height_m").to_pylist()[-2:], [3.0, 4.5])
        meta = t.schema.metadata
        self.assertEqual(meta[b"structures_contract"], b"structures_v2")
        self.assertEqual(meta[b"building_rows"], b"3")
        self.assertEqual(meta[b"barrier_rows"], b"2")

    def test_empty_cell_writes_a_zero_row_table(self):
        census, t = self.build([])
        self.assertEqual(t.num_rows, 0)
        self.assertEqual(t.column_names, CONTRACT.SCHEMA.names)
        self.assertEqual(t.schema.metadata[b"building_rows"], b"0")

    def test_idempotent_skip_and_input_refresh(self):
        buildings_arrow(self.prepared / SQUARE / "buildings.arrow",
                        [osm_row(0, OSM_POLY, 32.0)])
        rows = [ovt_row(OVT_LONELY)]
        census, _ = self.build(rows)
        self.assertIsNotNone(census)
        output = self.prepared / SQUARE / "structures.arrow"
        before = output.read_bytes()
        self.assertIsNone(BUILDER.build_square(
            SQUARE, self.prepared, rows, 0, FakeGlobalPrior(), None))
        self.assertEqual(output.read_bytes(), before)
        newer = output.stat().st_mtime + 5
        self.assertIsNotNone(BUILDER.build_square(
            SQUARE, self.prepared, rows, 0, FakeGlobalPrior(newer), None))
        source = self.prepared / SQUARE / "buildings.arrow"
        newer = output.stat().st_mtime + 5
        os.utime(source, (newer, newer))
        self.assertIsNotNone(BUILDER.build_square(
            SQUARE, self.prepared, rows, 0, FakeGlobalPrior(), None))

    def test_stale_buildings_contract_is_rejected(self):
        path = self.prepared / SQUARE / "buildings.arrow"
        buildings_arrow(path, [osm_row(0, OSM_POLY, 32.0)])
        # Re-stamp with an outdated contract: the merge must refuse to certify it.
        t = ipc.open_file(str(path)).read_all()
        schema = t.schema.with_metadata({b"buildings_contract": b"buildings_v1"})
        with ipc.new_file(str(path), schema) as w:
            w.write_table(t)
        with self.assertRaises(SystemExit):
            self.build([ovt_row(OVT_LONELY)])

    def test_grid_stamp_does_not_certify_float_or_null_coordinates(self):
        path = self.prepared / SQUARE / "buildings.arrow"
        for values, dtype in [([1.0], pa.float64()), ([None], pa.int32())]:
            with self.subTest(values=values):
                buildings_arrow(path, [osm_row(0, OSM_POLY, 32.0)])
                table = ipc.open_file(path).read_all()
                table = table.set_column(table.schema.get_field_index("centroid_gx"),
                                         "centroid_gx", pa.array(values, type=dtype))
                with ipc.new_file(path, table.schema) as writer:
                    writer.write_table(table)
                with self.assertRaisesRegex(SystemExit, "centroid_gx"):
                    self.build([])

    def test_old_producer_stamp_cannot_be_a_fresh_skip(self):
        self.build([ovt_row(OVT_LONELY)])
        path = self.prepared / SQUARE / "structures.arrow"
        table = ipc.open_file(path).read_all()
        metadata = dict(table.schema.metadata)
        metadata[b"builder_version"] = b"old-producer"
        table = table.replace_schema_metadata(metadata)
        with ipc.new_file(path, table.schema) as writer:
            writer.write_table(table)
        census, _ = self.build([ovt_row(OVT_LONELY)])
        self.assertIsNotNone(census)

    def test_wall_keeps_mapped_tier_and_wrapped_midpoint(self):
        path = self.prepared / SQUARE / "barriers.arrow"
        barriers_arrow(path, [{
            "osm_id": 55, "segment_idx": 0, "start_lat": 0.0, "start_lon": 179.999,
            "end_lat": 0.0, "end_lon": -179.999, "height": 3.0, "height_tier": 0}])
        table = ipc.open_file(path).read_all()
        _, output = self.build([])
        self.assertEqual(output.column("height_tier").to_pylist(), [0])
        lon, _ = GRID.grid_to_lonlat(output.column("centroid_gx")[0].as_py(),
                                     output.column("centroid_gy")[0].as_py())
        self.assertAlmostEqual(GRID.wrapped_longitude_delta(-180.0, lon), 0.0, places=5)
        (self.prepared / SQUARE / "structures.arrow").unlink()
        table = table.drop_columns(["height_tier"])
        with ipc.new_file(path, table.schema) as writer:
            writer.write_table(table)
        with self.assertRaisesRegex(SystemExit, "height_tier"):
            self.build([])


def overture_parquet(path, geometries):
    pq.write_table(pa.table({
        "geometry": pa.array([shapely.to_wkb(geometry) for geometry in geometries],
                             type=pa.binary())}), path)


class AntimeridianTests(unittest.TestCase):
    def test_footprint_centroid_crosses_the_dateline(self):
        # Same labelled footprint as the dev1 structure-builder regression.
        crossing = shapely.Polygon([
            (179.99, 49.7800), (-179.996, 49.7800),
            (-179.996, 49.7802), (179.99, 49.7802),
        ])
        lat, lon = SOURCES.footprint_centroid(crossing)
        self.assertAlmostEqual(lon, 179.997, places=9)
        self.assertAlmostEqual(lat, 49.7801, places=9)
        self.assertAlmostEqual(crossing.centroid.x, -0.003, places=9)
        ordinary = shapely.box(14.17000, 49.78000, 14.17020, 49.78016)
        self.assertEqual(SOURCES.footprint_centroid(ordinary),
                         (ordinary.centroid.y, ordinary.centroid.x))

    def test_parquet_ingest_owns_both_dateline_sides_exactly_once(self):
        east = shapely.Polygon([
            (179.99, 49.7800), (-179.996, 49.7800),
            (-179.996, 49.7802), (179.99, 49.7802),
        ])
        west = shapely.Polygon([
            (179.996, 49.7810), (-179.99, 49.7810),
            (-179.99, 49.7812), (179.996, 49.7812),
        ])
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            # The downloader retains each whole polygon in every bbox tile.
            # There is no E180 tile: longitude ownership is half-open.
            for name in ("N49E179.parquet", "N49W180.parquet"):
                overture_parquet(root / name, [east, west])
            east_rows, _ = BUILDER.read_overture_parquet(root, (511, 174))
            west_rows, _ = BUILDER.read_overture_parquet(root, (0, 174))
            self.assertEqual((len(east_rows), len(west_rows)), (1, 1))
            self.assertAlmostEqual(east_rows[0]["clon"], 179.997, places=9)
            self.assertAlmostEqual(west_rows[0]["clon"], -179.997, places=9)

    def test_missing_required_parquet_is_not_empty_coverage(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(SystemExit, "N49E179.parquet.*missing"):
                BUILDER.read_overture_parquet(directory, (511, 174))



if __name__ == "__main__":
    unittest.main(verbosity=2)
