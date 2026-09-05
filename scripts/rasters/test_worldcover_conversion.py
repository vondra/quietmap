"""WorldCover classes come from source pixels, never visualization overviews."""

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


class WorldCoverConversionTests(unittest.TestCase):
    def test_nearest_classes_ignore_source_overviews(self):
        script = Path(__file__).with_name("convert-worldcover.sh").read_text()
        function = "convert_one() {" + script.split("convert_one() {", 1)[1].split(
            "export -f convert_one", 1
        )[0]
        extent_script = Path(__file__).with_name("node-extent.sh").resolve()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, vrt = root / "worldcover.tif", root / "worldcover.vrt"
            values = np.full((512, 512), 10, dtype=np.uint8)
            values[259, 259] = 30  # Grass at target node row/column 1801.
            values[249, 249] = 80  # Water at target node row/column 1798.
            with rasterio.open(
                source, "w", driver="GTiff", width=512, height=512, count=1,
                dtype="uint8", crs="EPSG:4326",
                transform=Affine(1/12000, 0, 0.5-256/12000, 0, -1/12000, 0.5+256/12000),
            ) as dataset:
                dataset.write(values, 1)
                dataset.build_overviews([2, 4, 8], Resampling.nearest)
            with rasterio.open(source) as dataset:
                self.assertEqual(dataset.overviews(1), [2, 4, 8])
                self.assertEqual(dataset.read(1, out_shape=(256, 256))[129, 129], 10)
                self.assertEqual(dataset.read(1)[259, 259], 30)
            subprocess.run(["gdalbuildvrt", "-q", str(vrt), str(source)], check=True)
            with rasterio.open(vrt) as dataset:
                self.assertIn(2, dataset.overviews(1), "fixture must expose the misleading overview")
            forest, imd = root / "forest", root / "imd"
            forest.mkdir()
            imd.mkdir()
            failures = root / "failures"
            failures.touch()
            environment = {
                **os.environ, "VRT": str(vrt), "VRT_DIR": directory,
                "FOREST_DST": str(forest), "IMD_DST": str(imd), "IMD_FORCE": "0",
                "FAIL_LIST": str(failures), "QM_VENV_PYTHON": sys.executable,
            }
            result = subprocess.run(
                ["bash", "-euo", "pipefail", "-c",
                 'source "$1"\n' + function + "\nconvert_one N00E000", "_", str(extent_script)],
                env=environment, capture_output=True, text=True, check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(failures.read_text(), "")
            for output, expected in [(forest, [0, 0, 100]), (imd, [5, 100, 2])]:
                values = np.fromfile(output / "N00E000.raw", dtype=np.uint8).reshape(3601, 3601)
                actual = [int(values[row, col]) for row, col in [(1801, 1801), (1798, 1798), (1801, 1798)]]
                self.assertEqual(actual, expected, output.name)


if __name__ == "__main__":
    unittest.main()
