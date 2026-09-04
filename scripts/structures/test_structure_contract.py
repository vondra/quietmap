"""Prepared heights and complete input identity cannot silently change popup physics."""

from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest

import pyarrow as pa
import pyarrow.ipc as ipc

from test_structures_fixtures import (
    BUILDER, CONTRACT, SQUARE, FakeGlobalPrior, buildings_arrow, barriers_arrow,
    osm_row, ovt_row, OSM_POLY, OVT_LONELY,
)


class PreparedStructureContractTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.prepared = Path(self.temporary.name)
        self.square = self.prepared / SQUARE
        self.square.mkdir(parents=True)

    def build(self, regional=None):
        return BUILDER.build_square(
            SQUARE, self.prepared, [ovt_row(OVT_LONELY)], 0,
            FakeGlobalPrior(), regional)

    def test_prepared_height_rounds_once_without_changing_raw_emission(self):
        buildings_arrow(self.square / "buildings.arrow",
                        [osm_row(0, OSM_POLY, 32.0, height=4.5)])
        barriers_arrow(self.square / "barriers.arrow", [{
            "osm_id": 77, "segment_idx": 0, "start_lat": 49.78,
            "start_lon": 14.17, "end_lat": 49.7801, "end_lon": 14.1701,
            "height": 2.5, "height_tier": 0}])
        self.build()
        table = ipc.open_file(self.square / "structures.arrow").read_all()
        self.assertEqual(table.schema.field("height_m").type, pa.int16())
        self.assertEqual(table.column("height_m").to_pylist(), [5, 13, 3])
        self.assertEqual(table.column("height").to_pylist(), [4.5, None, None])
        self.assertEqual(table.column("height_tier").to_pylist(), [0, 4, 0])

    def test_height_quantization_is_bounded_and_refuses_invalid_source_values(self):
        values = [0.0, 0.49, 0.5, 2.5, 4.5, 12.49, 32767.0]
        quantized = [CONTRACT.screening_height_metres(value) for value in values]
        self.assertEqual(quantized, [0, 0, 1, 3, 5, 12, 32767])
        self.assertTrue(all(abs(old - new) <= 0.5 for old, new in zip(values, quantized)))
        for invalid in [-0.1, 32768.0, float("nan"), float("inf")]:
            with self.subTest(height=invalid), self.assertRaises(ValueError):
                CONTRACT.screening_height_metres(invalid)

    def test_deleted_source_retracts_its_old_structures(self):
        buildings_arrow(self.square / "buildings.arrow",
                        [osm_row(0, OSM_POLY, 32.0, height=4.5)])
        self.build()
        (self.square / "buildings.arrow").unlink()
        self.assertIsNotNone(self.build())
        table = ipc.open_file(self.square / "structures.arrow").read_all()
        self.assertEqual(table.num_rows, 1)
        self.assertEqual(table.column("osm_id").null_count, 1)

    def test_regional_selection_is_an_input_even_when_its_mtime_is_older(self):
        regional = SimpleNamespace(
            mtime=0, input_identity="regional-measurement",
            tr=SimpleNamespace(transform=lambda xs, ys: (xs, ys)),
            covers=lambda _x, _y: True,
            zonal_measured_mean=lambda _polygon: 20.5)
        self.build()
        self.assertIsNotNone(self.build(regional))
        table = ipc.open_file(self.square / "structures.arrow").read_all()
        self.assertEqual(table.column("height_tier").to_pylist(), [3])
        self.assertIsNotNone(self.build())
        table = ipc.open_file(self.square / "structures.arrow").read_all()
        self.assertEqual(table.column("height_tier").to_pylist(), [4])


if __name__ == "__main__":
    unittest.main(verbosity=2)
