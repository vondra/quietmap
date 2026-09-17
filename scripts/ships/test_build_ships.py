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

    def test_dataset_id_matches_the_generated_registry(self):
        # pipeline/lib/sources.ts is the registry; its generated TypeScript mirror pins the id.
        generated = (Path(__file__).resolve().parents[2] / "pipeline/lib/source-ids.generated.ts").read_text()
        self.assertIn(f"= {build_ships.SOURCE_ID_EMODNET_2024} as const // emodnet-vessel-density-2024", generated)

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


class GfwReaderTests(unittest.TestCase):
    def test_gfw_tiles_become_cells_outside_the_emodnet_coverage(self):
        import io
        import json
        import zipfile
        import rasterio
        from rasterio.transform import from_origin

        def tile_zip(path, values):
            if values is None:
                path.write_bytes(b"")
                return
            buffer = io.BytesIO()
            with rasterio.open(buffer, "w", driver="GTiff", width=3, height=2, count=1, dtype="int32",
                               crs="EPSG:4326", transform=from_origin(4.0, 52.02, 0.01, 0.01), nodata=build_ships.GFW_NODATA) as dataset:
                dataset.write(np.asarray(values, dtype=np.int32), 1)
            with zipfile.ZipFile(path, "w") as archive:
                archive.writestr(build_ships.GFW_TIF_MEMBER, buffer.getvalue())

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "window.json").write_text(json.dumps({"days": 365}))
            # large: 3650 h/365 d = 300.4 h/month in cell (0,0); nodata elsewhere but one 1-hour cell (dropped: < 0.5 h/month)
            tile_zip(root / "lon+000_lat+48-large.zip", [[3650, build_ships.GFW_NODATA, 1], [build_ships.GFW_NODATA] * 3])
            tile_zip(root / "lon+000_lat+48-work.zip", [[365, 730, build_ships.GFW_NODATA], [build_ships.GFW_NODATA] * 3])
            tile_zip(root / "lon+008_lat+48-large.zip", None)  # empty tile: no reports at all
            tile_zip(root / "lon+008_lat+48-work.zip", None)
            cells = build_ships.read_gfw(root)
            self.assertEqual(len(cells["lon"]), 2)
            self.assertAlmostEqual(float(cells["lon"][0]), 4.005)
            self.assertAlmostEqual(float(cells["lat"][0]), 52.015)
            self.assertAlmostEqual(float(cells["hours_large"][0]), 3650 * 30.4375 / 365, places=6)
            self.assertAlmostEqual(float(cells["hours_work"][0]), 30.4375, places=6)
            self.assertAlmostEqual(float(cells["hours_work"][1]), 60.875, places=6)
            self.assertEqual(float(cells["hours_leisure"].sum()), 0.0)
            self.assertTrue(all(cells["source_id"] == build_ships.SOURCE_ID_GFW_PRESENCE))
            self.assertAlmostEqual(cells["raster_hours_kept"], (3650 + 365 + 730) * 30.4375 / 365, places=6)
            self.assertAlmostEqual(cells["raster_hours_total"], (3650 + 1 + 365 + 730) * 30.4375 / 365, places=6)
            # cell area: 0.01° × 0.01° at 52°N
            self.assertAlmostEqual(float(cells["area_m2"][0]) / 1e6, (1113.2 ** 2) * np.cos(np.radians(52.015)) / 1e6, places=3)
            # a coverage that samples the first cell centre removes it
            coverage = {"mask": np.array([[True]]), "crs": "EPSG:4326",
                        "affine": from_origin(4.0, 52.02, 0.01, 0.01)}
            self.assertEqual(len(build_ships.read_gfw(root, exclude=coverage)["lon"]), 1)
