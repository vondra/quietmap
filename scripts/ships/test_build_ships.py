"""ships.arrow keeps every cell's hours, prunes by z14 block and matches the engine grid."""

import base64
from pathlib import Path
import struct
import sys
import tempfile
import unittest

import numpy as np
import pyarrow as pa

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
import build_ships  # noqa: E402
import qmgrid  # noqa: E402


def cells(lon, lat, large, work=None, leisure=None):
    n = len(lon)
    return {
        "lon": np.asarray(lon, dtype=np.float64), "lat": np.asarray(lat, dtype=np.float64),
        "area_m2": np.full(n, 1e6), "hours_large": np.asarray(large, dtype=np.float64),
        "hours_work": np.zeros(n) if work is None else np.asarray(work, dtype=np.float64),
        "hours_leisure": np.zeros(n) if leisure is None else np.asarray(leisure, dtype=np.float64),
        "source_id": np.full(n, build_ships.SOURCE_ID_EMODNET_2024, dtype=np.uint16),
    }


class BuildShipsTests(unittest.TestCase):
    def test_squares_blocks_and_grid_match_the_engine(self):
        # Two cells share the Rotterdam-approach square; Scheveningen and Gibraltar have their own.
        data = cells([4.0, 4.01, 4.4, -5.6], [52.0, 52.0, 52.3, 36.0], [10.0, 20.0, 5.0, 348.0], [1, 0, 0, 2])
        with tempfile.TemporaryDirectory() as directory:
            prepared = Path(directory)
            stale = prepared / "z9/1/1/ships.arrow"
            stale.parent.mkdir(parents=True)
            stale.write_bytes(b"old")
            report = build_ships.write_prepared(data, prepared)
            self.assertEqual(report["squares"], 3)
            self.assertEqual(report["rows"], 4)
            self.assertEqual(report["removed_stale_squares"], 1)
            self.assertFalse(stale.exists())
            self.assertAlmostEqual(report["hours_written"], 386.0)
            sx, sy = qmgrid.square_of(52.0, 4.0)
            path = prepared / qmgrid.square_name(sx, sy) / "ships.arrow"
            reader = pa.ipc.open_file(pa.memory_map(str(path), "r"))
            metadata = reader.schema.metadata
            self.assertEqual(metadata[b"ships_contract"], b"ships_v1")
            self.assertEqual(metadata[b"grid"], b"z30")
            blocks = base64.b64decode(metadata[b"qm_blocks"])
            self.assertEqual(blocks[0], 1)
            records = [build_ships.BLOCK_RECORD.unpack_from(blocks, 1 + i * build_ships.BLOCK_RECORD.size)
                       for i in range((len(blocks) - 1) // build_ships.BLOCK_RECORD.size)]
            self.assertEqual(len(records), reader.num_record_batches)
            total = 0
            for record, index in zip(records, range(reader.num_record_batches)):
                batch = reader.get_batch(index)
                total += batch.num_rows
                lon, lat = zip(*(qmgrid.grid_to_lonlat(gx, gy) for gx, gy in
                                 zip(batch.column("centroid_gx").to_pylist(), batch.column("centroid_gy").to_pylist())))
                self.assertLessEqual(record[2], min(lat) + 1e-6)
                self.assertGreaterEqual(record[4], max(lat) - 1e-6)
                self.assertLessEqual(record[3], min(lon) + 1e-6)
                self.assertGreaterEqual(record[5], max(lon) - 1e-6)
                self.assertEqual(record[6:], (0.0, 0.0))
            self.assertEqual(total, 2)
            gx, gy = qmgrid.lonlat_to_grid(4.0, 52.0)
            table = reader.read_all()
            row = table.to_pylist()[[c["hours_large"] for c in table.to_pylist()].index(10.0)]
            self.assertEqual((row["centroid_gx"], row["centroid_gy"]), (gx, gy))
            self.assertEqual(row["hours_work"], 1.0)
            self.assertEqual(row["source_id"], build_ships.SOURCE_ID_EMODNET_2024)

    def test_batches_split_at_the_block_row_cap(self):
        n = build_ships.MAX_ROWS_PER_BLOCK_BATCH + 1
        lon = np.full(n, 4.0) + np.arange(n) * 1e-7  # one z14 cell
        data = cells(lon, np.full(n, 52.0), np.ones(n))
        table, batches = build_ships.square_table(data, np.arange(n))
        self.assertEqual([length for _, length in batches], [build_ships.MAX_ROWS_PER_BLOCK_BATCH, 1])
        self.assertEqual(table.num_rows, n)
        self.assertEqual(struct.unpack_from("<HH", base64.b64decode(table.schema.metadata[b"qm_blocks"]), 1),
                         tuple(int(v[0]) for v in build_ships.mercator_axes([52.0], [4.0], 14)))


if __name__ == "__main__":
    unittest.main()
