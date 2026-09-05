"""DEM failures must preserve old data and never publish partial height grids."""

from pathlib import Path
import os
import re
import sqlite3
import subprocess
import tempfile
import unittest

import numpy as np
import rasterio
from rasterio.transform import Affine


SCRIPT = Path(__file__).with_name("convert-dem-copernicus.sh")
SOURCE_NAME = "Copernicus_DSM_COG_10_N49_00_E016_00_DEM"
GRID_BYTES = 2 * 3601 * 3601


def write_source(root: Path):
    source = root / "source" / "glo30" / SOURCE_NAME / f"{SOURCE_NAME}.tif"
    source.parent.mkdir(parents=True)
    with rasterio.open(
        source, "w", driver="GTiff", width=8, height=8, count=1,
        dtype="int16", crs="EPSG:4326",
        transform=Affine(0.25, 0, 15.875, 0, -0.25, 50.125),
    ) as dataset:
        dataset.write(np.full((8, 8), 1234, dtype=np.int16), 1)


def run_conversion(root: Path, shims: dict[str, str], coverage_gaps: bool = False):
    output = root / "output"
    scratch = root / "scratch"
    binaries = root / "bin"
    output.mkdir(exist_ok=True)
    scratch.mkdir(exist_ok=True)
    binaries.mkdir(exist_ok=True)
    inventories = {30: set(), 90: set()}
    for source in (root / "source").rglob("*_DEM.tif"):
        resolution = 30 if "_COG_10_" in source.name else 90
        inventories[resolution].add(source.stem)
    inventories[90].update(name.replace("_COG_10_", "_COG_30_") for name in inventories[30])
    catalog = root / "source" / "catalog.sqlite"
    if not catalog.exists():
        with sqlite3.connect(catalog) as database:
            database.execute("CREATE TABLE catalog (resolution INTEGER PRIMARY KEY, inventory TEXT NOT NULL)")
            database.executemany("INSERT INTO catalog VALUES (?, ?)",
                                 [(resolution, "\n".join(sorted(names))) for resolution, names in inventories.items()])
    for name in ["gdalbuildvrt", "python3"]:
        executable = binaries / name
        if name in shims:
            only_writer = '[[ "$1" == *dem_native_grid.py ]] || exec /usr/bin/python3 "$@"\n' if name == "python3" else ""
            executable.write_text("#!/usr/bin/env bash\nset -euo pipefail\n" + only_writer + shims[name])
            executable.chmod(0o755)
        elif executable.exists():
            executable.unlink()
    environment = {
        **os.environ,
        "DEM_SRC": str(root / "source"),
        "DEM_DST": str(output),
        "TMPDIR": str(scratch),
        "JOBS": "2",
        "PATH": str(binaries) + os.pathsep + os.environ["PATH"],
    }
    return subprocess.run(
        ["bash", str(SCRIPT.resolve()), *(["--coverage-gaps"] if coverage_gaps else [])],
        env=environment, capture_output=True, text=True, check=False,
    )


class DemConversionTests(unittest.TestCase):
    def test_native_nodes_and_periodic_interpolation_survive_mixed_resolutions(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for latitude, longitude, width, values in [
                (0, 10, 3600, 1000.5 + 2 * (np.arange(3600) % 100)),
                (-1, 10, 1800, 3000 + 0.75 * np.arange(1800)),
                (-89, 179, 360, 100 + 20 * np.arange(360)),
                (-89, -180, 360, 7300 + 20 * np.arange(360)),
            ]:
                if latitude == -1:
                    values[100:102] = [2289.911376953125, 2291.08837890625]
                elif latitude == 0:
                    values[50:53] = [-32768, -1.5, 0]
                ns, ew = "N" if latitude >= 0 else "S", "E" if longitude >= 0 else "W"
                name = f"Copernicus_DSM_COG_10_{ns}{abs(latitude):02}_00_{ew}{abs(longitude):03}_00_DEM"
                source = root / "source" / "glo30" / name / f"{name}.tif"
                source.parent.mkdir(parents=True)
                with rasterio.open(
                    source, "w", driver="GTiff", width=width, height=3600, count=1,
                    dtype="float32", crs="EPSG:4326", compress="deflate",
                    transform=Affine(1/width, 0, longitude-0.5/width,
                                     0, -1/3600, latitude+1+0.5/3600),
                ) as dataset:
                    dataset.write(np.broadcast_to(values.astype(np.float32), (3600, width)), 1)
            result = run_conversion(root, {})
            self.assertEqual(result.returncode, 0, result.stderr)
            output = root / "output"
            fine = np.fromfile(output / "N00E010.hgt", dtype=">i2").reshape(3601, 3601)
            expected = 1001 + 2 * (np.arange(1740, 1861) % 100)
            np.testing.assert_array_equal(fine[1800, 1740:1861], expected)
            np.testing.assert_array_equal(fine[1800, 49:53], [1099, -32768, -2, 0])
            south = np.fromfile(output / "S01E010.hgt", dtype=">i2").reshape(3601, 3601)
            np.testing.assert_array_equal(fine[-1], south[0])
            native_south = 3000 + 0.75 * np.arange(1800)
            native_south[100:102] = [2289.911376953125, 2291.08837890625]
            expected_south = np.floor(np.interp(np.arange(3599)/2, np.arange(1800), native_south) + 0.5)
            np.testing.assert_array_equal(south[0, :3599], expected_south)
            np.testing.assert_array_equal(fine[0, 1740:1861], expected)
            self.assertTrue(np.all(south[-1] == -32768))
            west = np.fromfile(output / "S89E179.hgt", dtype=">i2").reshape(3601, 3601)
            east = np.fromfile(output / "S89W180.hgt", dtype=">i2").reshape(3601, 3601)
            np.testing.assert_array_equal(west[:3600, -1], east[:3600, 0])
            self.assertTrue(np.all(west[:3600, -1] == 7300))
            # Check the interpolation support before the shared node, not only edge equality.
            expected = 100 + 2 * np.arange(3588, 3601)
            np.testing.assert_array_equal(west[1800, 3588:], expected)
            self.assertTrue(np.all(west[-1] == -32768))

    def test_official_coverage_gap_keeps_fine_nodes_and_both_shared_axes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for resolution, cells in [
                (30, [(latitude, longitude) for latitude in (39, 40, 41)
                      for longitude in (39, 40, 41) if (latitude, longitude) != (40, 40)]),
                (90, [(latitude, longitude) for latitude in (39, 40) for longitude in (40, 41)]),
            ]:
                size, code = (3600, 10) if resolution == 30 else (1200, 30)
                for latitude, longitude in cells:
                    name = f"Copernicus_DSM_COG_{code}_N{latitude:02}_00_E{longitude:03}_00_DEM"
                    source = root / "source" / f"glo{resolution}" / name / f"{name}.tif"
                    source.parent.mkdir(parents=True, exist_ok=True)
                    y, x = np.indices((size, size), dtype=np.float32)
                    values = 8000.5 + (longitude-40)*3600 + x*3600/size + (latitude+1-40)*3600 - y*3600/size
                    if resolution == 30:
                        values += 100
                    elif (latitude, longitude) == (40, 40):
                        values[500:600, 500:600] = 1000.5
                        values[500:600, 550] = 1000.4999389648438
                        values[700, 700] = -32768
                    with rasterio.open(
                        source, "w", driver="GTiff", width=size, height=size, count=1,
                        dtype="float32", crs="EPSG:4326", compress="deflate",
                        transform=Affine(1/size, 0, longitude-0.5/size,
                                         0, -1/size, latitude+1+0.5/size),
                    ) as dataset:
                        dataset.write(values, 1)
            result = run_conversion(root, {})
            self.assertEqual(result.returncode, 0, result.stderr)
            tiles = {}
            for path in (root / "output").glob("*.hgt"):
                match = re.fullmatch(r"N(\d{2})E(\d{3})", path.stem)
                assert match is not None
                latitude, longitude = map(int, match.groups())
                tile = np.fromfile(path, dtype=">i2").reshape(3601, 3601)
                tiles[latitude, longitude] = tile
                y, x = np.indices((3600, 3600), dtype=np.int16)
                expected = 8001 + (longitude-40)*3600 + x + (latitude+1-40)*3600 - y
                if (latitude, longitude) != (40, 40):
                    expected += 100
                else:
                    # Exact 3:1 node weights, independently of the production sampler.
                    halo = np.empty((1201, 1201), dtype=np.float64)
                    for dy, dx in [(0, 0), (0, 1), (1, 0), (1, 1)]:
                        name = f"Copernicus_DSM_COG_30_N{40-dy:02}_00_E{40+dx:03}_00_DEM"
                        with rasterio.open(root / "source" / "glo90" / name / f"{name}.tif") as source:
                            values = source.read(1)
                        halo[1200*dy:1200*dy+(1 if dy else 1200),
                             1200*dx:1200*dx+(1 if dx else 1200)] = values[:1 if dy else 1200, :1 if dx else 1200]
                    columns = np.arange(3600) // 3
                    remainder = np.arange(3600) % 3
                    expected = np.empty((3600, 3600), dtype=np.int16)
                    for row in range(3600):
                        native_row, ry = divmod(row, 3)
                        top = halo[native_row, columns]*(3-remainder) + halo[native_row, columns+1]*remainder
                        bottom = halo[native_row+1, columns]*(3-remainder) + halo[native_row+1, columns+1]*remainder
                        expected[row] = np.floor((top*(3-ry)+bottom*ry)/9 + 0.5)
                        for iy, wy in [(native_row, 3-ry), (native_row+1, ry)]:
                            for ix, wx in [(columns, 3-remainder), (columns+1, remainder)]:
                                expected[row, (halo[iy, ix] == -32768) & (wy*wx != 0)] = -32768
                np.testing.assert_array_equal(tile[:-1, :-1], expected)
            self.assertEqual(len(tiles), 9)
            for (latitude, longitude), tile in tiles.items():
                if (latitude, longitude+1) in tiles:
                    np.testing.assert_array_equal(tile[:, -1], tiles[latitude, longitude+1][:, 0])
                if (latitude+1, longitude) in tiles:
                    np.testing.assert_array_equal(tile[0], tiles[latitude+1, longitude][-1])
            # Complete-size outputs in the dependency closure must not skip the repair.
            output_paths = list((root / "output").glob("*.hgt"))
            before = {path: path.stat().st_mtime_ns for path in output_paths}
            core = root / "output" / "N40E040.hgt"
            expected_core = core.read_bytes()
            core.write_bytes(bytes(GRID_BYTES))
            repaired = run_conversion(root, {}, coverage_gaps=True)
            self.assertEqual(repaired.returncode, 0, repaired.stderr)
            self.assertEqual(core.read_bytes(), expected_core)
            changed = {path.stem for path in output_paths if path.stat().st_mtime_ns != before[path]}
            self.assertEqual(changed, {"N40E040", "N40E039", "N41E040", "N41E039"})
            missing = next((root / "source" / "glo90").rglob("*N40_00_E040_00_DEM.tif"))
            missing.rename(root / "retained-source.tif")
            before = {path: path.stat().st_mtime_ns for path in output_paths}
            failed = run_conversion(root, {}, coverage_gaps=True)
            self.assertNotEqual(failed.returncode, 0)
            self.assertIn("Missing catalogued GLO90", failed.stderr)
            self.assertEqual({path: path.stat().st_mtime_ns for path in output_paths}, before)

    def test_failure_never_publishes_or_overwrites_output(self):
        failures = {
            "mosaic": ({"gdalbuildvrt": "exit 44"}, 44),
            "writer": (
                {"python3": 'printf partial > "${@: -1}"; exit 43'},
                123,
            ),
            "short_output": (
                {"python3": 'printf short > "${@: -1}"'},
                123,
            ),
        }
        for failure, (shims, expected_status) in failures.items():
            for existing in [False, True]:
                with self.subTest(failure=failure, existing=existing):
                    with tempfile.TemporaryDirectory() as directory:
                        root = Path(directory)
                        write_source(root)
                        output = root / "output" / "N49E016.hgt"
                        output.parent.mkdir()
                        if existing:
                            output.write_bytes(b"retained")
                        result = run_conversion(root, shims)
                        self.assertEqual(result.returncode, expected_status, result.stderr)
                        if existing:
                            self.assertEqual(output.read_bytes(), b"retained")
                        else:
                            self.assertFalse(output.exists())
                        self.assertEqual(list((root / "scratch").iterdir()), [])
                        self.assertEqual(list(output.parent.iterdir()), [output] if existing else [])

    def test_resume_rebuilds_incomplete_output_and_skips_complete_output(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_source(root)
            output = root / "output" / "N49E016.hgt"
            output.parent.mkdir()
            output.write_bytes(b"incomplete")
            result = run_conversion(
                root,
                {"python3": f'truncate -s {GRID_BYTES} "${{@: -1}}"'},
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(output.read_bytes(), bytes(GRID_BYTES))
            resumed = run_conversion(root, {"python3": "exit 42"})
            self.assertEqual(resumed.returncode, 0, resumed.stderr)
            self.assertEqual(output.read_bytes(), bytes(GRID_BYTES))
            self.assertEqual(list((root / "scratch").iterdir()), [])
            self.assertEqual(list(output.parent.iterdir()), [output])

    def test_real_gdal_publishes_big_endian_height_grid(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_source(root)
            result = run_conversion(root, {})
            self.assertEqual(result.returncode, 0, result.stderr)
            output = root / "output" / "N49E016.hgt"
            self.assertEqual(output.stat().st_size, GRID_BYTES)
            heights = np.fromfile(output, dtype=">i2")
            self.assertTrue(np.all(heights == 1234))
            self.assertEqual(list((root / "scratch").iterdir()), [])
            self.assertEqual(list(output.parent.iterdir()), [output])


if __name__ == "__main__":
    unittest.main()
