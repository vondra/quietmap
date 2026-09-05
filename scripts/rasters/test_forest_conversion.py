"""Continuous canopy averages native measurements and preserves Hansen gaps."""

import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

import numpy as np
import rasterio
from rasterio.enums import Resampling
from rasterio.transform import Affine


class ForestConversionTests(unittest.TestCase):
    def test_density_averages_native_pixels_not_overviews(self):
        script = Path(__file__).with_name("convert-forest-continuous.sh").read_text()
        function = "convert_one()" + script.split("convert_one()", 1)[1].split(
            "export -f convert_one", 1
        )[0]
        extent_script = Path(__file__).with_name("node-extent.sh").resolve()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for layer, value in [("tc", 80), ("ly", 0)]:
                with rasterio.open(
                    root / f"{layer}.tif", "w", driver="GTiff", width=4, height=4,
                    count=1, dtype="uint8", crs="EPSG:4326",
                    transform=Affine(0.25, 0, 0, 0, -0.25, 1),
                ) as dataset:
                    dataset.write(np.full((4, 4), value, dtype=np.uint8), 1)
            tcd, vrt = root / "tcd.tif", root / "tcd.vrt"
            values = np.zeros((512, 512), dtype=np.uint8)
            values[259, 259] = 100
            with rasterio.open(
                tcd, "w", driver="GTiff", width=512, height=512, count=1,
                dtype="uint8", crs="EPSG:4326", nodata=255,
                transform=Affine(1/12000, 0, 0.5-256/12000, 0, -1/12000, 0.5+256/12000),
            ) as dataset:
                dataset.write(values, 1)
                dataset.build_overviews([2, 4, 8], Resampling.nearest)
            subprocess.run(["gdalbuildvrt", "-q", "-srcnodata", "255", "-vrtnodata", "255",
                            str(vrt), str(tcd)], check=True)
            with rasterio.open(vrt) as dataset:
                self.assertIn(2, dataset.overviews(1), "fixture must expose an overview")
                self.assertEqual(dataset.read(1, out_shape=(256, 256))[129, 129], 0)
                self.assertEqual(dataset.read(1)[259, 259], 100)
            output = root / "forest"
            output.mkdir()
            environment = {
                **os.environ, "FOREST_DST": str(output), "GRID": "3601",
                "EXPECTED_BYTES": str(3601 * 3601), "TCD_VRT": str(vrt),
                "TC_VRT": str(root / "tc.tif"), "LY_VRT": str(root / "ly.tif"),
                "QM_VENV_PYTHON": sys.executable, "TMPDIR": directory,
            }
            result = subprocess.run(
                ["bash", "-euo", "pipefail", "-c",
                 'source "$1"\n' + function + "\nconvert_one N00E000", "_", str(extent_script)],
                env=environment, capture_output=True, text=True, check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            actual = np.fromfile(output / "N00E000.raw", dtype=np.uint8).reshape(3601, 3601)
            # One 100% source pixel occupies 1/(12000/3600)^2 of this target cell.
            self.assertEqual(actual[1801, 1801], 9)
            self.assertEqual(actual[1000, 1000], 80, "outside TCD must retain Hansen")


if __name__ == "__main__":
    unittest.main()
