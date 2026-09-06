"""Regressions for dev1 geography semantics and lossless z9 Arrow country baking."""

import os
from pathlib import Path
import struct
import tempfile
import unittest

import numpy as np
import pyarrow as pa

from admin_at import AdminResolver
from build_admin import bake_file, baked_batch, segment_midpoints, square_admin, write_admin_record
from qmgrid import lonlat_to_grid, square_id


def country_feature(group, west, south, east, north):
    return {"properties": {"shapeGroup": group}, "geometry": {"type": "Polygon", "coordinates": [
        [[west, south], [east, south], [east, north], [west, north], [west, south]]
    ]}}


def segment_batch(points):
    grid = [lonlat_to_grid(lon, lat) for lat, lon in points]
    columns = {"osm_id": pa.array(range(len(points)), type=pa.int64())}
    for prefix in ("start", "end"):
        for index, axis in enumerate(("gx", "gy")):
            columns[prefix + "_" + axis] = pa.array([p[index] for p in grid], type=pa.int32())
    return pa.record_batch(columns)


def write_prepared_admin_roundtrip(directory):
    feature = country_feature("CZE", 14, 49, 15, 51)
    geography = {"countries": {"CZE": ["CZ", 1]}, "metros": [
        {"id": 31, "country": "CZ", "polygon": feature["geometry"]["coordinates"][0]}
    ]}
    resolver = AdminResolver([feature], geography)
    square_directory = Path(directory) / "z9/276/174"
    square_directory.mkdir(parents=True)
    write_admin_record(square_directory, square_admin(resolver, 276, 174))


class CountryBakeTests(unittest.TestCase):
    def setUp(self):
        self.resolver = AdminResolver([country_feature("CZE", 14, 49, 15, 51),
                                       country_feature("POL", 15, 49, 16, 51)])

    def test_each_segment_keeps_its_country_across_a_partition_border(self):
        batch = segment_batch([(50, 14.8), (50, 15.2)])
        baked = baked_batch(batch, self.resolver, b"roads_contract")
        self.assertEqual(baked.column("country_iso").to_pylist(),
                         [int.from_bytes(b"CZ", "little"), int.from_bytes(b"PL", "little")])
        for name in batch.schema.names:
            self.assertTrue(batch.column(name).equals(baked.column(name)))

    def test_arrow_rewrite_preserves_spatial_batches_and_is_idempotent(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "roads.arrow"
            metadata = {b"qm_batch_bboxes": b"[[49,14,51,15],[49,15,51,16]]", b"source": b"fixture"}
            batches = [segment_batch([(50, 14.8)]).replace_schema_metadata(metadata),
                       segment_batch([(50, 15.2)]).replace_schema_metadata(metadata)]
            with pa.ipc.new_file(path, batches[0].schema) as writer:
                for batch in batches:
                    writer.write_batch(batch)
            self.assertEqual(bake_file(path, self.resolver), (2, True))
            before = path.read_bytes()
            with pa.ipc.open_file(path) as reader:
                self.assertEqual(reader.num_record_batches, 2)
                self.assertEqual(reader.schema.metadata[b"qm_batch_bboxes"], metadata[b"qm_batch_bboxes"])
                self.assertEqual(reader.schema.metadata[b"source"], b"fixture")
            self.assertEqual(bake_file(path, self.resolver), (2, False))
            self.assertEqual(path.read_bytes(), before)

    def test_industrial_ownership_is_strict_land_with_holes_and_unchanged_road_coastal_policy(self):
        feature = country_feature("CZE", 14, 49, 15, 51)
        feature["geometry"]["coordinates"].append([[14.4, 49.4], [14.6, 49.4],
                                                   [14.6, 49.6], [14.4, 49.6], [14.4, 49.4]])
        resolver = AdminResolver([feature])
        points = [(50, 14.5), (50, 15.001), (49.5, 14.5)]
        grid = [lonlat_to_grid(lon, lat) for lat, lon in points]
        batch = pa.record_batch({"centroid_gx": pa.array([p[0] for p in grid], type=pa.int32()),
                                 "centroid_gy": pa.array([p[1] for p in grid], type=pa.int32()),
                                 "source_id": pa.array([330, 330, 330], type=pa.uint16())})
        batch = batch.replace_schema_metadata({b"grid": b"z30", b"native": b"retain"})
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "industrial.arrow"
            with pa.ipc.new_file(path, batch.schema) as writer:
                writer.write_batch(batch.slice(0, 1)); writer.write_batch(batch.slice(1))
            self.assertEqual(bake_file(path, resolver), (3, True))
            with pa.ipc.open_file(path) as reader:
                self.assertEqual(reader.num_record_batches, 2)
                self.assertEqual(reader.schema.metadata[b"industrial_contract"], b"country_land_baked_v1")
                result = reader.read_all()
                self.assertEqual(result.column("country_iso").to_pylist(), [int.from_bytes(b"CZ", "little"), 0, 0])
                for name in batch.schema.names:
                    self.assertEqual(result.column(name).to_pylist(), batch.column(name).to_pylist())
            old = path.read_bytes(), path.stat()
            self.assertEqual(bake_file(path, resolver), (3, False))
            self.assertEqual(path.read_bytes(), old[0]); self.assertEqual(path.stat(), old[1])
        road = baked_batch(segment_batch(points), resolver, b"roads_contract")
        self.assertEqual(road.column("country_iso")[1].as_py(), int.from_bytes(b"CZ", "little"))

    def test_partial_country_columns_leave_original_file_untouched(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "railways.arrow"
            batch = segment_batch([(50, 14.8)]).append_column("country_iso", pa.array([0], type=pa.uint16()))
            with pa.ipc.new_file(path, batch.schema) as writer:
                writer.write_batch(batch)
            before = path.read_bytes()
            with self.assertRaisesRegex(ValueError, "all-or-none"):
                bake_file(path, self.resolver)
            self.assertEqual(path.read_bytes(), before)
            self.assertEqual(list(Path(directory).iterdir()), [path])

    def test_midpoint_takes_the_short_arc_at_the_antimeridian(self):
        batch = segment_batch([(0, 179.9)])
        gx, gy = lonlat_to_grid(-179.9, 0)
        batch = batch.set_column(batch.schema.get_field_index("end_gx"), "end_gx", pa.array([gx], type=pa.int32()))
        batch = batch.set_column(batch.schema.get_field_index("end_gy"), "end_gy", pa.array([gy], type=pa.int32()))
        lat, lon = segment_midpoints(batch)
        self.assertLess(abs(abs(lon[0]) - 180), 1e-6)
        self.assertLess(abs(lat[0]), 1e-6)

    def test_record_matches_rust_morton_and_binary_layout(self):
        self.assertEqual(square_id(276, 173), 100786)
        record = square_admin(self.resolver, 276, 173)
        self.assertEqual(len(record), 13)
        stored, continent, country, _ = struct.unpack("<QBHH", record)
        self.assertEqual((stored, continent, country), (100786, 1, int.from_bytes(b"CZ", "little")))
        with tempfile.TemporaryDirectory() as directory:
            square_directory = Path(directory) / "z9/276/173"
            square_directory.mkdir(parents=True)
            write_admin_record(square_directory, record)
            self.assertEqual((square_directory / "admin.bin").read_bytes(), record)

    def test_unmapped_disputed_land_never_inherits_a_neighbour(self):
        resolver = AdminResolver([country_feature("CZE", 14, 49, 15, 51),
                                  country_feature("999", 15, 49, 16, 51)])
        result = resolver.resolve([50, 50, np.nan], [15.001, 15.5, 14.5])
        self.assertEqual(result["country_iso"].tolist(), [0, 0, 0])
        disputed = AdminResolver([country_feature("111", 14, 49, 15, 51)])
        self.assertEqual(disputed.resolve([50], [14.5])["country_iso"].tolist(), [int.from_bytes(b"SD", "little")])
        self.assertEqual(disputed.resolve_land([50], [14.5])["country_iso"].tolist(), [0])
        with self.assertRaisesRegex(ValueError, "Unmapped CGAZ"):
            AdminResolver([country_feature("ZZZ", 14, 49, 15, 51)])


@unittest.skipUnless(os.environ.get("QM_ADMIN_BOUNDARIES"), "set QM_ADMIN_BOUNDARIES for real CGAZ regression coverage")
class CgazRegressionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.resolver = AdminResolver.from_file(os.environ["QM_ADMIN_BOUNDARIES"])

    def test_dev1_border_microstate_coast_seam_and_polar_reference_points(self):
        # Same independently labelled coordinates as dev1 admin-at.test.ts.
        cases = [(50.087, 14.421, "CZ"), (1.352, 103.82, "SG"), (9.705, 100.017, "TH"),
                 (49.98953, 18.1288, "CZ"), (-29.31, 27.48, "LS"), (43.7384, 7.4246, "MC"),
                 (41.9029, 12.4534, "VA"), (43.9424, 12.4578, "SM"), (0, -30, ""),
                 (-45, -140, ""), (1.452, 103.768, ""), (9.72, 99.975, "TH"),
                 (9.72, 99.95, ""), (-18.1416, 178.4419, "FJ"), (-16.83, -179.97, "FJ"),
                 (-16.5, 180, "FJ"), (-80, 120, "AQ"), (-85, 50, "AQ"), (-80, 0, "AQ"),
                 (-66, 110, ""), (-60, -70, "")]
        result = self.resolver.resolve([p[0] for p in cases], [p[1] for p in cases])
        for index, (lat, lon, expected) in enumerate(cases):
            with self.subTest(lat=lat, lon=lon):
                self.assertEqual(result["country_iso"][index], int.from_bytes(expected.encode(), "little"))

    def test_city_requires_its_own_country(self):
        result = self.resolver.resolve([13.75, 49], [100.5, 10])
        self.assertEqual(result["city_id"].tolist(), [22, 0])
        self.assertEqual(self.resolver.city_ids([13.75], [100.5], np.array([int.from_bytes(b"KH", "little")])).tolist(), [0])


if __name__ == "__main__":
    unittest.main()
