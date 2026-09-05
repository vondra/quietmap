"""IMD regressions exercise actual GDAL mosaics, projection, and atomic output."""

from contextlib import ExitStack
import errno
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock
import xml.etree.ElementTree as ET

import numpy as np
import rasterio
from rasterio.transform import Affine
from rasterio.warp import transform_bounds

import imd_overlay as overlay


def write_source(path, values, crs="EPSG:4326", transform=None):
    path.parent.mkdir(parents=True, exist_ok=True)
    with rasterio.open(path, "w", driver="GTiff", width=values.shape[1],
                       height=values.shape[0], count=1, dtype="uint8", crs=crs,
                       transform=transform or Affine(0.5, 0, -0.25, 0, -0.5, 1.25)) as source:
        source.write(values, 1)


class ImdOverlayTests(unittest.TestCase):
    def test_special_codes_are_masked_before_interpolation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, masked = root / "source.tif", root / "masked.vrt"
            values = np.array([[255, 254, 0], [0, 100, 70], [20, 80, 100]], dtype=np.uint8)
            write_source(source, values)
            overlay.masked_source_vrt(source, masked)
            rectangles = {node.tag: node.attrib for node in ET.parse(masked).iter()
                          if node.tag in ("SrcRect", "DstRect")}
            expected_rectangle = {"xOff": "0", "yOff": "0", "xSize": "3", "ySize": "3"}
            self.assertEqual(rectangles, {"SrcRect": expected_rectangle,
                                         "DstRect": expected_rectangle})
            with rasterio.open(masked) as dataset:
                np.testing.assert_array_equal(dataset.read(1),
                                              np.where(values > 100, 255, values))
            with overlay.open_mosaic(masked, 5) as dataset:
                actual = overlay.overlay_tile(np.full((5, 5), 5, dtype=np.uint8),
                                              [(dataset, {"N00E000"})], "N00E000")
            self.assertEqual(actual[0, 0], 5)
            self.assertEqual(actual[0, 2], 5)
            self.assertEqual(actual[0, 3], 0)  # No 254-to-zero contamination.
            self.assertEqual(actual[0, 4], 0)

    def test_mixed_projections_latest_zero_and_original_water_win(self):
        with tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
            root = Path(directory)
            bounds = (14, 50, 15, 51)
            for year, crs, value in [(2018, "EPSG:3857", 100), (2024, "EPSG:3035", 0)]:
                left, bottom, right, top = transform_bounds("EPSG:4326", crs, *bounds)
                values = np.full((100, 100), value, dtype=np.uint8)
                if year == 2024:
                    values[:, 60:] = 255
                write_source(root / str(year) / "source.tif", values, crs,
                             Affine((right-left)/100, 0, left, 0, (bottom-top)/100, top))
            temporary = root / "temporary"
            temporary.mkdir()
            mosaics = overlay.build_mosaics(root, temporary)
            self.assertEqual(len(mosaics), 2)
            opened = [(stack.enter_context(overlay.open_mosaic(path, 11)), coverage)
                      for path, coverage in mosaics]
            base = np.full((11, 11), 5, dtype=np.uint8)
            base[5, 4] = 100
            actual = overlay.overlay_tile(base, opened, "N50E014")
            self.assertEqual(actual[5, 3], 0)  # New natural ground replaces old urban 100.
            self.assertEqual(actual[5, 4], 100)  # Only original water stays hard.
            self.assertEqual(actual[5, 8], 100)  # New nodata retains older measurement.
            cli_warp = root / "system-gdal-point.tif"
            subprocess.run(
                ["gdalwarp", "-q", "-t_srs", "EPSG:4326", "-te",
                 "14.299", "50.499", "14.301", "50.501", "-ts", "1", "1",
                 "-r", "bilinear", "-srcnodata", "255", "-dstnodata", "255",
                 str(mosaics[-1][0]), str(cli_warp)], check=True,
            )
            with rasterio.open(cli_warp) as dataset:
                self.assertEqual(int(dataset.read(1)[0, 0]), 0)

    def test_uncovered_base_is_unchanged_and_zero_tile_is_published(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            base, output, sources = root / "base", root / "output", root / "sources"
            base.mkdir()
            np.full((3, 3), 85, dtype=np.uint8).tofile(base / "N00E000.raw")
            untouched = base / "N20E020.raw"
            untouched.write_bytes(bytes([5]) * 9)
            latest = sources / "2024" / "source.tif"
            write_source(latest, np.full((3, 3), 100, dtype=np.uint8))
            with mock.patch.object(overlay, "GRID", 3):
                changed = overlay.convert(sources, base, output, ["N00E000", "N20E020"])
                self.assertEqual(changed, 2)
                self.assertEqual((output / "N00E000.raw").read_bytes(), bytes([100]) * 9)
                write_source(latest, np.zeros((3, 3), dtype=np.uint8))
                self.assertEqual(overlay.convert(sources, base, output,
                                                 ["N00E000", "N20E020"]), 1)
                self.assertEqual((output / "N00E000.raw").read_bytes(), bytes(9))
                self.assertEqual((output / "N20E020.raw").read_bytes(), untouched.read_bytes())
                self.assertEqual(overlay.convert(sources, base, output,
                                                 ["N00E000", "N20E020"]), 0)
            self.assertEqual((base / "N00E000.raw").read_bytes(), bytes([85]) * 9)

    def test_atomic_failure_retains_old_output_and_cleans_temporary(self):
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "N00E000.raw"
            destination.write_bytes(b"old")
            with mock.patch.object(overlay.os, "replace", side_effect=OSError("injected")):
                with self.assertRaisesRegex(OSError, "injected"):
                    overlay.publish_atomic(destination, np.zeros((3, 3), dtype=np.uint8))
            self.assertEqual(destination.read_bytes(), b"old")
            self.assertEqual(list(Path(directory).iterdir()), [destination])

    def test_copy_fallback_and_invalid_base_fail_loudly(self):
        with tempfile.TemporaryDirectory() as directory:
            source, output = Path(directory) / "base.raw", Path(directory) / "out.raw"
            source.write_bytes(bytes([5]) * 9)
            with mock.patch.object(overlay.fcntl, "ioctl", side_effect=OSError(errno.EXDEV, "different disk")):
                self.assertTrue(overlay.publish_atomic(output, source))
            self.assertEqual(output.read_bytes(), source.read_bytes())
            source.write_bytes(b"bad")
            with self.assertRaisesRegex(ValueError, "tile length"):
                overlay.read_base(source, 3)
            source.write_bytes(bytes([255]) * 9)
            with self.assertRaisesRegex(ValueError, "outside"):
                overlay.read_base(source, 3)
            with self.assertRaisesRegex(ValueError, "differ"):
                overlay.convert(Path(directory), Path(directory), Path(directory))

    def test_source_or_warp_failure_cannot_report_success(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "2024" / "source.tif"
            write_source(source, np.zeros((3, 3), dtype=np.uint8))
            with mock.patch.object(overlay.subprocess, "run", side_effect=subprocess.CalledProcessError(1, "gdalbuildvrt")):
                with self.assertRaises(subprocess.CalledProcessError):
                    overlay.build_mosaics(root, root)
            with mock.patch.object(overlay, "WarpedVRT", side_effect=RuntimeError("warp failed")):
                with self.assertRaisesRegex(RuntimeError, "warp failed"):
                    with overlay.open_mosaic(source, 3):
                        pass

    def test_projected_sampling_is_independent_of_tile_and_read_window(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source.tif"
            left, bottom, right, top = transform_bounds(
                "EPSG:4326", "EPSG:3035", 13.9, 49.9, 16.1, 52.1)
            values = np.random.default_rng(2024).integers(0, 101, (512, 512), dtype=np.uint8)
            write_source(source, values, "EPSG:3035",
                         Affine((right-left)/512, 0, left, 0, (bottom-top)/512, top))
            names = ["N50E014", "N50E015", "N51E014"]
            with overlay.open_mosaic(source, 101) as mosaic:
                base = np.full((101, 101), 5, dtype=np.uint8)
                first, east, north = [overlay.overlay_tile(base, [(mosaic, set(names))], name)
                                      for name in names]
                np.testing.assert_array_equal(first[:, -1], east[:, 0])
                np.testing.assert_array_equal(first[0, :], north[-1, :])
                pieces = np.vstack([
                    mosaic.read(1, window=((3900+row, 3900+min(row+17, 101)),
                                           (19400, 19501)))
                    for row in range(0, 101, 17)
                ])
                np.testing.assert_array_equal(first, pieces)
                self.assertGreater(np.count_nonzero(first != 5), 10000)
                self.assertLessEqual(first.max(), 100)

    def test_common_sampling_preserves_an_analytic_linear_field(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source.tif"
            cols = 13.95 + (np.arange(840)+0.5)*0.0025
            rows = 52.05 - (np.arange(840)+0.5)*0.0025
            values = np.rint(30+(cols[None, :]-14)*10+(rows[:, None]-50)*15).astype(np.uint8)
            write_source(source, values, transform=Affine(0.0025, 0, 13.95, 0, -0.0025, 52.05))
            with overlay.open_mosaic(source, 101) as mosaic:
                actual = overlay.overlay_tile(np.full((101, 101), 5, dtype=np.uint8),
                                              [(mosaic, {"N50E014"})], "N50E014")
            longitude = 14 + np.arange(101)/100
            latitude = 51 - np.arange(101)/100
            expected = np.rint(30+(longitude[None, :]-14)*10+(latitude[:, None]-50)*15)
            # Integer source and output quantization can straddle a rounding tie.
            self.assertLessEqual(np.abs(actual.astype(np.int16)-expected).max(), 1)


if __name__ == "__main__":
    unittest.main()
