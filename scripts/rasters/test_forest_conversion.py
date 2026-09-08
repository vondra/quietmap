"""Continuous canopy averages native measurements and preserves Hansen gaps."""

import hashlib
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
    def convert(self, root, tile, treecover, lossyear, tcd):
        script = Path(__file__).with_name("convert-forest-continuous.sh").read_text()
        function = "convert_one()" + script.split("convert_one()", 1)[1].split(
            "export -f convert_one", 1
        )[0]
        extent_script = Path(__file__).with_name("node-extent.sh").resolve()
        output = root / "forest"
        output.mkdir(parents=True, exist_ok=True)
        environment = {
            **os.environ, "FOREST_DST": str(output), "GRID": "3601",
            "EXPECTED_BYTES": str(3601 * 3601), "TCD_VRT": str(tcd),
            "TC_VRT": str(treecover), "LY_VRT": str(lossyear),
            "QM_VENV_PYTHON": sys.executable, "TMPDIR": str(root),
        }
        result = subprocess.run(
            ["bash", "-euo", "pipefail", "-c",
             'source "$1"\n' + function + '\nconvert_one "$2"', "_", str(extent_script), tile],
            env=environment, capture_output=True, text=True, check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return np.fromfile(output / f"{tile}.raw", dtype=np.uint8).reshape(3601, 3601)

    def test_density_averages_native_pixels_not_overviews(self):
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
            actual = self.convert(root, "N00E000", root / "tc.tif", root / "ly.tif", vrt)
            # One 100% source pixel occupies 1/(12000/3600)^2 of this target cell.
            self.assertEqual(actual[1801, 1801], 9)
            self.assertEqual(actual[1000, 1000], 80, "outside TCD must retain Hansen")

    def test_average_quantization_is_half_up_and_partition_independent(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            native, zero, vrt = root / "native.tif", root / "zero.tif", root / "native.vrt"
            for path in (native, zero):
                with rasterio.open(
                    path, "w", driver="GTiff", width=40000, height=40000,
                    count=1, dtype="uint8", crs="EPSG:4326", tiled=True,
                    sparse_ok=True, compress="DEFLATE",
                    transform=Affine(1/4000, 0, -110, 0, -1/4000, 60),
                ) as dataset:
                    if path == native:
                        dataset.write(np.array([[70, 45], [70, 45]], dtype=np.uint8), 1,
                                      window=((15999, 16001), (19787, 19789)))
            subprocess.run(["gdalbuildvrt", "-q", "-te", "-180", "-60", "180", "80",
                            str(vrt), str(native)], check=True)
            for layer in ("hansen", "tcd"):
                with self.subTest(layer=layer):
                    treecover = vrt if layer == "hansen" else zero
                    tcd = vrt if layer == "tcd" else root / "absent.vrt"
                    south = self.convert(root / layer, "N55W106", treecover, zero, tcd)
                    north = self.convert(root / layer, "N56W106", treecover, zero, tcd)
                    # Native weights 7/20, 3/20, 7/20, 3/20 give exactly 62.5%.
                    self.assertEqual(south[0, 3409], 63)
                    self.assertEqual(north[-1, 3409], 63)
                    np.testing.assert_array_equal(south[0], north[-1])

            # The public CLI must normalize display metadata, never source measurements.
            with rasterio.open(native, "r+") as dataset:
                dataset.write_colormap(1, {0: (255, 0, 0, 255), 45: (0, 0, 255, 255),
                                          70: (0, 255, 0, 255)})
            source_hashes = {path: hashlib.sha256(path.read_bytes()).digest()
                             for path in (native, zero)}
            tcd_dir, hansen_dir, output = root / "palette-tcd", root / "hansen", root / "cli"
            tcd_dir.mkdir()
            (tcd_dir / "tcd.tif").symlink_to(native)
            for layer in ("treecover2000", "lossyear"):
                (hansen_dir / layer).mkdir(parents=True)
                (hansen_dir / layer / f"Hansen_GFC-2024-v1.12_{layer}_60N_110W.tif").symlink_to(zero)
            result = subprocess.run(
                ["bash", str(Path(__file__).with_name("convert-forest-continuous.sh")),
                 "N55W106", "N56W106"],
                env={**os.environ, "TCD_DIR": str(tcd_dir), "HANSEN_DIR": str(hansen_dir),
                     "FOREST_DST": str(output), "QM_VENV_PYTHON": sys.executable,
                     "TMPDIR": str(root)}, capture_output=True, text=True, check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stderr, "")
            for path, checksum in source_hashes.items():
                self.assertEqual(hashlib.sha256(path.read_bytes()).digest(), checksum)
            for tile in ("N55W106", "N56W106"):
                self.assertEqual((output / f"{tile}.raw").read_bytes(),
                                 (root / "tcd" / "forest" / f"{tile}.raw").read_bytes())


if __name__ == "__main__":
    unittest.main()
