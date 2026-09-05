"""Source-only structures and a terminal resume share one complete world inventory."""

import io
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import numpy as np
import pyarrow as pa
import pyarrow.ipc as ipc
import pyarrow.parquet as pq
import rasterio
from affine import Affine

from test_structures_fixtures import (
    BUILDER, GRID, SQUARE, FakeGlobalPrior, OVT_LONELY, ovt_row,
)


class StructureInventoryTests(unittest.TestCase):
    def test_source_only_square_and_complete_world_resume(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            prepared = root / "prepared"
            prepared.mkdir()
            rows = [ovt_row(OVT_LONELY)]
            census = BUILDER.build_square(
                SQUARE, prepared, rows, [], FakeGlobalPrior(), None)
            self.assertEqual(census["overture_only"], 1)
            self.assertFalse((prepared / SQUARE / "buildings.arrow").exists())
            table = ipc.open_file(prepared / SQUARE / "structures.arrow").read_all()
            self.assertEqual(table.column("osm_id").to_pylist(), [None])

            prepared = root / "world-prepared"
            prepared.mkdir()
            parquet = root / "parquet"
            parquet.mkdir()
            empty = root / "empty.parquet"
            pq.write_table(pa.table({"geometry": pa.array([], type=pa.binary())}), empty)
            for lat in range(-90, 90):
                for lon in range(-180, 180):
                    name = (f"{'N' if lat >= 0 else 'S'}{abs(lat):02d}"
                            f"{'E' if lon >= 0 else 'W'}{abs(lon):03d}.parquet")
                    os.link(empty, parquet / name)
            occupied = parquet / "N49E014.parquet"
            occupied.unlink()
            pq.write_table(pa.table({"geometry": pa.array([rows[0]["wkb"]],
                                                          type=pa.binary())}), occupied)
            prior = root / "prior.tif"
            with rasterio.open(prior, "w", driver="GTiff", width=1, height=1,
                               count=1, dtype="float32", crs="EPSG:4326",
                               transform=Affine(360, 0, -180, 0, -180, 90)) as writer:
                writer.write(np.array([[12.5]], dtype=np.float32), 1)
            # A prepared square without any building remains part of the output
            # inventory because another layer can require its obstacle table.
            other = GRID.square_name(*GRID.square_of(-41.3, 174.8))
            (prepared / other).mkdir(parents=True)
            selected = BUILDER.world_squares(parquet)
            self.assertEqual(len(selected), GRID.Z9_AXIS ** 2)
            self.assertEqual(len(set(selected)), len(selected))
            self.assertEqual(selected[0], GRID.square_name(0, 0))
            self.assertEqual(selected[-1], GRID.square_name(GRID.Z9_AXIS - 1, GRID.Z9_AXIS - 1))
            # Prove the full selection above; execute only these three real outputs.
            empty_ocean = GRID.square_name(*GRID.square_of(0.0, -140.0))
            args = ["build-structures.py", "--squares", ",".join([SQUARE, other, empty_ocean]),
                    "--prepared-dir", str(prepared),
                    "--overture-parquet", str(parquet), "--ghsl", str(prior)]
            with patch("sys.argv", args), patch("sys.stdout", new_callable=io.StringIO):
                BUILDER.main()
            self.assertEqual(ipc.open_file(prepared / other / "structures.arrow")
                             .read_all().num_rows, 0)
            outputs = list(prepared.glob("z9/*/*/structures.arrow"))
            self.assertEqual(len(outputs), 3)
            before = {path: (path.read_bytes(), path.stat().st_mtime_ns) for path in outputs}
            self.assertEqual(sum(ipc.open_file(path).read_all().num_rows for path in outputs), 1)
            with (patch("sys.argv", args), patch("sys.stdout", new_callable=io.StringIO),
                  patch.object(BUILDER, "read_overture_parquet",
                               side_effect=AssertionError("fresh source decoded again"))):
                BUILDER.main()
            self.assertEqual(before, {path: (path.read_bytes(), path.stat().st_mtime_ns)
                                      for path in outputs})
            world_args = [args[0], *args[3:]]  # Default CLI path, without --squares.
            malformed = prepared / "z9/0276/173"
            malformed.mkdir(parents=True)
            with (patch("sys.argv", world_args),
                  self.assertRaisesRegex(ValueError, "Noncanonical prepared square")):
                BUILDER.main()
            malformed.rmdir()
            self.assertEqual(before, {path: (path.read_bytes(), path.stat().st_mtime_ns)
                                      for path in prepared.glob("z9/*/*/structures.arrow")})
            # Missing cached coverage is not certified as an empty ocean tile.
            (parquet / "N52W179.parquet").unlink()
            with (patch("sys.argv", world_args), self.assertRaisesRegex(ValueError, "N52W179")):
                BUILDER.main()
            self.assertEqual(before, {path: (path.read_bytes(), path.stat().st_mtime_ns)
                                      for path in prepared.glob("z9/*/*/structures.arrow")})


if __name__ == "__main__":
    unittest.main(verbosity=2)
