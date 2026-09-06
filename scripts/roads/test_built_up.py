"""Real-IPC regressions for density stock, cell coverage and lossless road enrichment."""

import math
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import pyarrow as pa

from building_footprints import BuildingFootprintSampler, WINDOW_HALF_DEG
from build_built_up import bake_file
import qmgrid

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "structures"))
from structure_contract import SCHEMA  # noqa: E402

STRUCTURES_SCHEMA = SCHEMA.with_metadata({b"grid": b"z30", b"structures_contract": b"structures_v4"})


def ring(lat, lon, side):
    dlat = side / 2 / 111_132
    dlon = side / 2 / (111_320 * math.cos(math.radians(lat)))
    return [qmgrid.lonlat_to_grid(qmgrid.normalize_longitude(x), y)
            for x, y in [(lon - dlon, lat - dlat), (lon + dlon, lat - dlat),
                         (lon + dlon, lat + dlat), (lon - dlon, lat + dlat)]]


def footprint(lat, lon, side=100, stock="overture", holes=(), extra_parts=()):
    gx, gy = qmgrid.lonlat_to_grid(lon, lat)
    return {"kind": 0, "geom": qmgrid.encode_grid_polygons(
                [[ring(lat, lon, side), *(ring(lat, lon, hole) for hole in holes)],
                 *([ring(lat, lon, part)] for part in extra_parts)]),
            "height_m": 8, "height_tier": 2, "envelope_class": 5,
            "centroid_gx": gx, "centroid_gy": gy,
            "osm_id": None if stock == "overture" else 123,
            "emission_centroid_gx": gx if stock == "matched" else None,
            "emission_centroid_gy": gy if stock == "matched" else None}


def write_structures(root, square, rows, schema=STRUCTURES_SCHEMA):
    path = root / qmgrid.square_name(*square) / "structures.arrow"
    path.parent.mkdir(parents=True, exist_ok=True)
    table = pa.Table.from_pylist(rows, schema=schema)
    with pa.ipc.new_file(path, schema) as writer:
        for batch in table.to_batches(max_chunksize=2):
            writer.write_batch(batch)
    return path


def road_batch(points, metadata=None):
    columns = {"osm_id": pa.array(range(len(points)), type=pa.int64()),
               "geom": pa.array([b"original geometry"] * len(points), type=pa.binary()),
               "country_iso": pa.array([int.from_bytes(b"CZ", "little")] * len(points), type=pa.uint16()),
               "road_class": pa.array([3] * len(points), type=pa.uint8()),
               "maxspeed": pa.array([None] * len(points), type=pa.uint16())}
    for prefix in ("start", "end"):
        grid = [qmgrid.lonlat_to_grid(lon, lat) for lat, lon in points]
        for index, axis in enumerate(("gx", "gy")):
            columns[prefix + "_" + axis] = pa.array([p[index] for p in grid], type=pa.int32())
    batch = pa.record_batch(columns)
    field = batch.schema.field("geom").with_metadata({b"encoding": b"untouched"})
    batch = batch.set_column(batch.schema.get_field_index("geom"), field, batch.column("geom"))
    return batch.replace_schema_metadata(metadata or {b"grid": b"z30"})


def write_roads(path, batches):
    path.parent.mkdir(parents=True, exist_ok=True)
    with pa.ipc.new_file(path, batches[0].schema) as writer:
        for batch in batches:
            writer.write_batch(batch)
    path.chmod(0o640)


class BuiltUpTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.point = (49.5, 14.5)
        self.square = qmgrid.square_of(*self.point)

    def sampler(self):
        return BuildingFootprintSampler(self.root)

    def test_degree_window_uses_centroids_in_both_axes(self):
        lat, lon = self.point
        inside, outside = WINDOW_HALF_DEG * .9, WINDOW_HALF_DEG * 1.1
        rows = [footprint(lat + dy, lon + dx, 20)
                for dy, dx in [(0, 0), (inside, 0), (0, inside), (outside, 0), (0, outside)]]
        write_structures(self.root, self.square, rows)
        area = self.sampler().window_area_m2(lat, lon)
        assert area is not None
        self.assertAlmostEqual(area, 1200, delta=5)

    def test_courtyards_and_multipart_area_cross_the_calibrated_threshold(self):
        lat, lon = self.point
        for holes, parts, expected_area, expected_class in [((80,), (), 3600, 1),
                                                            ((80,), (40,), 5200, 2),
                                                            ((50,), (), 7500, 2)]:
            with self.subTest(holes=holes, parts=parts):
                write_structures(self.root, self.square,
                                 [footprint(lat, lon, holes=holes, extra_parts=parts)])
                sampler = self.sampler()
                area = sampler.window_area_m2(lat, lon)
                assert area is not None
                self.assertAlmostEqual(area, expected_area, delta=10)
                self.assertEqual(sampler.classify(lat, lon), expected_class)

    def test_only_overture_stock_counts_even_with_osm_and_barrier_rows(self):
        lat, lon = self.point
        rows = [footprint(lat, lon, 30), footprint(lat, lon, 30, "matched"),
                footprint(lat, lon, 200, "osm")]
        barrier = footprint(lat, lon, 200)
        barrier.update(kind=1, geom=qmgrid.encode_grid_poly(ring(lat, lon, 200)[:2]))
        rows.append(barrier)
        write_structures(self.root, self.square, rows)
        sampler = self.sampler()
        area = sampler.window_area_m2(lat, lon)
        assert area is not None
        self.assertAlmostEqual(area, 1800, delta=5)
        self.assertEqual(sampler.classify(lat, lon), 1)

    def test_four_cell_corner_requires_every_owner_even_when_known_area_is_urban(self):
        squares = [(255, 255), (256, 255), (255, 256), (256, 256)]
        for square, point in zip(squares, [(0.001, -.001), (.001, .001), (-.001, -.001), (-.001, .001)]):
            self.assertEqual(qmgrid.square_of(*point), square)
            write_structures(self.root, square, [footprint(*point, side=70)])
        self.assertEqual(self.sampler().classify(0, 0), 2)
        (self.root / "z9/255/255/structures.arrow").unlink()
        self.assertEqual(self.sampler().classify(0, 0), 0)
        write_structures(self.root, (255, 255), [])
        self.assertEqual(self.sampler().classify(0, 0), 2)

    def test_owner_corner_working_set_loads_once_and_stays_bounded_when_owner_changes(self):
        owner = (256, 256)
        west, north, east, south = qmgrid.square_lonlat_span(*owner)
        inset = WINDOW_HALF_DEG / 4
        corners = [(north - inset, west + inset), (north - inset, east - inset),
                   (south + inset, west + inset), (south + inset, east - inset)]
        for x in range(255, 258):
            for y in range(255, 258):
                write_structures(self.root, (x, y), [])
        sides = [20, 30, 40, 50]
        write_structures(self.root, owner,
                         [footprint(*point, side=side) for point, side in zip(corners, sides)])
        sampler = self.sampler()
        with patch.object(sampler, "load_cell", wraps=sampler.load_cell) as load:
            for index in [0, 1, 2, 3, 0]:
                area = sampler.window_area_m2(*corners[index])
                assert area is not None
                self.assertAlmostEqual(area, sides[index] ** 2, delta=5)
                self.assertLessEqual(len(sampler.cells), 9)
            self.assertEqual(load.call_count, 9)
            self.assertEqual(len({call.args[0] for call in load.call_args_list}), 9)
            write_structures(self.root, self.square, [])
            self.assertEqual(sampler.classify(*self.point), 1)
            self.assertEqual(load.call_count, 10)
            self.assertEqual(len(sampler.cells), 9)
            area = sampler.window_area_m2(*corners[0])
            assert area is not None
            self.assertAlmostEqual(area, sides[0] ** 2, delta=5)
            self.assertEqual(len(sampler.cells), 9)

    def test_dateline_footprint_and_road_midpoint_preserve_arrow_identity(self):
        y = qmgrid.square_of(10, 179.99)[1]
        write_structures(self.root, (511, y), [footprint(10, 179.9999)])
        write_structures(self.root, (0, y), [])
        area = self.sampler().window_area_m2(10, 180)
        assert area is not None
        self.assertAlmostEqual(area, 10000, delta=15)
        metadata = {b"grid": b"z30", b"roads_contract": b"country_baked_v1",
                    b"qm_batch_bboxes": b"[[9,179,11,180],[49,14,50,15]]", b"source": b"fixture"}
        first = road_batch([(10, 179.9999)], metadata)
        gx, _ = qmgrid.lonlat_to_grid(-179.9999, 10)
        index = first.schema.get_field_index("end_gx")
        first = first.set_column(index, first.schema.field(index), pa.array([gx], type=pa.int32()))
        second = road_batch([self.point, self.point], metadata)
        write_structures(self.root, self.square, [])
        path = self.root / f"z9/511/{y}/roads.arrow"
        write_roads(path, [first, second])
        result = bake_file(path, self.sampler())
        self.assertEqual(result, {"rows": 3, "unknown": 0, "rural": 2, "urban": 1, "files_changed": 1})
        with pa.ipc.open_file(path) as reader:
            self.assertEqual(reader.num_record_batches, 2)
            self.assertEqual(reader.schema.metadata, metadata)
            self.assertEqual(reader.get_batch(0).column("built_up").to_pylist(), [2])
            for i, original in enumerate((first, second)):
                batch = reader.get_batch(i)
                for field in original.schema:
                    self.assertEqual(batch.schema.field(field.name), field)
                    self.assertTrue(batch.column(field.name).equals(original.column(field.name)))
        before = path.read_bytes(), path.stat()
        self.assertEqual(bake_file(path, self.sampler())["files_changed"], 0)
        self.assertEqual(path.read_bytes(), before[0])
        self.assertEqual(path.stat(), before[1])
        self.assertEqual(path.stat().st_mode & 0o777, 0o640)

    def test_absent_valid_empty_and_corrupt_structures_are_distinct(self):
        self.assertEqual(self.sampler().classify(*self.point), 0)
        path = write_structures(self.root, self.square, [])
        with pa.ipc.open_file(path) as reader:
            self.assertEqual(reader.num_record_batches, 0)
        self.assertEqual(self.sampler().classify(*self.point), 1)
        write_structures(self.root, self.square, [], STRUCTURES_SCHEMA.remove_metadata())
        with self.assertRaisesRegex(ValueError, "structures_v4"):
            self.sampler().classify(*self.point)
        path.write_bytes(b"corrupt IPC")
        with self.assertRaises(pa.ArrowInvalid):
            self.sampler().classify(*self.point)

    def test_late_bad_geometry_never_replaces_the_original_road_file(self):
        write_structures(self.root, self.square, [])
        point = (30, 30)
        broken = footprint(*point)
        broken["geom"] = b"truncated multipart"
        write_structures(self.root, qmgrid.square_of(*point), [broken])
        path = self.root / qmgrid.square_name(*self.square) / "roads.arrow"
        write_roads(path, [road_batch([self.point]), road_batch([point])])
        before = path.read_bytes(), path.stat()
        with self.assertRaisesRegex(ValueError, "Malformed structures_v4"):
            bake_file(path, self.sampler())
        self.assertEqual((path.read_bytes(), path.stat()), before)
        self.assertEqual(sorted(p.name for p in path.parent.iterdir()), ["roads.arrow", "structures.arrow"])

    def test_cli_rejects_unbuilt_scope_and_reports_partial_coverage_honestly(self):
        path = self.root / qmgrid.square_name(*self.square) / "roads.arrow"
        write_roads(path, [road_batch([self.point, (30, 30)])])
        command = [sys.executable, str(Path(__file__).with_name("build_built_up.py")),
                   "--prepared-dir", str(self.root), "--square", qmgrid.square_name(*self.square)]
        before = path.read_bytes()
        missing = subprocess.run(command, text=True, capture_output=True)
        self.assertNotEqual(missing.returncode, 0)
        self.assertIn("No structures.arrow", missing.stderr)
        self.assertEqual(path.read_bytes(), before)
        write_structures(self.root, self.square, [])
        built = subprocess.run(command, text=True, capture_output=True)
        self.assertEqual(built.returncode, 0, built.stderr)
        self.assertIn('"unknown": 1', built.stdout)
        self.assertIn('"rural": 1', built.stdout)


if __name__ == "__main__":
    unittest.main()
