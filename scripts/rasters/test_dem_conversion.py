"""DEM failures must preserve old data and never publish partial height grids."""

from pathlib import Path
import os
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
    source = root / "source" / SOURCE_NAME / f"{SOURCE_NAME}.tif"
    source.parent.mkdir(parents=True)
    with rasterio.open(
        source, "w", driver="GTiff", width=8, height=8, count=1,
        dtype="int16", crs="EPSG:4326",
        transform=Affine(0.25, 0, 15.5, 0, -0.25, 50.5),
    ) as dataset:
        dataset.write(np.full((8, 8), 1234, dtype=np.int16), 1)


def run_conversion(root: Path, shims: dict[str, str]):
    output = root / "output"
    scratch = root / "scratch"
    binaries = root / "bin"
    output.mkdir(exist_ok=True)
    scratch.mkdir(exist_ok=True)
    binaries.mkdir(exist_ok=True)
    for name in ["gdalbuildvrt", "gdalwarp", "gdal_translate"]:
        executable = binaries / name
        if name in shims:
            executable.write_text("#!/usr/bin/env bash\nset -euo pipefail\n" + shims[name])
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
        ["bash", str(SCRIPT.resolve())],
        env=environment, capture_output=True, text=True, check=False,
    )


class DemConversionTests(unittest.TestCase):
    def test_failure_never_publishes_or_overwrites_output(self):
        failures = {
            "mosaic": ({"gdalbuildvrt": "exit 44"}, 44),
            "warp": ({"gdalwarp": "exit 42"}, 123),
            "translate": (
                {"gdalwarp": "exit 0",
                 "gdal_translate": 'printf partial > "${@: -1}"; exit 43'},
                123,
            ),
            "short_output": (
                {"gdalwarp": "exit 0",
                 "gdal_translate": 'printf short > "${@: -1}"'},
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
                {"gdalwarp": "exit 0",
                 "gdal_translate": f'truncate -s {GRID_BYTES} "${{@: -1}}"'},
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(output.read_bytes(), bytes(GRID_BYTES))
            resumed = run_conversion(root, {"gdalwarp": "exit 42"})
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
