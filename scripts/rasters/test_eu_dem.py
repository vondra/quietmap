"""Unit tests for the EU DEM fetchers, windows and planner (no network)."""
import importlib.util
import json
import math
import tempfile
import unittest
from pathlib import Path

import numpy as np
from osgeo import gdal, osr

from dem_windows import (LandMask, grid_windows, latitude_of_square_edge, reproject_bounds,
                         split_wcs_multipart, square_bounds, square_of_latitude,
                         square_of_longitude)

gdal.UseExceptions()
osr.UseExceptions()


def load_hyphenated(name):
    path = Path(__file__).with_name(name + '.py')
    spec = importlib.util.spec_from_file_location(name.replace('-', '_'), path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class GridWindowsTest(unittest.TestCase):
    def test_aligned_cover(self):
        windows = grid_windows(0, 0, 20000, 10000, 10000)
        self.assertEqual(windows, [(0, 0, 10000, 10000), (10000, 0, 20000, 10000)])

    def test_snaps_outward(self):
        windows = grid_windows(5000, 5000, 15000, 15000, 10000)
        self.assertEqual(windows, [(0, 0, 10000, 10000), (0, 10000, 10000, 20000),
                                   (10000, 0, 20000, 10000), (10000, 10000, 20000, 20000)])

    def test_rejects_empty(self):
        with self.assertRaises(ValueError):
            grid_windows(0, 0, 0, 10000, 10000)


class ReprojectBoundsTest(unittest.TestCase):
    def test_lambert93_paris(self):
        west, south, east, north = reproject_bounds(652000, 6860000, 653000, 6861000, 2154)
        self.assertAlmostEqual((west + east) / 2, 2.35, places=1)
        self.assertAlmostEqual((south + north) / 2, 48.85, places=1)


class SplitMultipartTest(unittest.TestCase):
    def test_extracts_tiff_part(self):
        body = (b'--wcs\r\nContent-Type: text/xml\r\n\r\n<gml/>\r\n'
                b'--wcs\r\nContent-Type: image/tiff\r\n\r\nII*\x00payload\r\n--wcs--\r\n')
        self.assertEqual(split_wcs_multipart(body, 'multipart/related; boundary=wcs'),
                         b'II*\x00payload')

    def test_missing_tiff_raises(self):
        body = b'--wcs\r\nContent-Type: text/xml\r\n\r\n<err/>\r\n--wcs--\r\n'
        with self.assertRaises(ValueError):
            split_wcs_multipart(body, 'multipart/related; boundary=wcs')


class LandMaskTest(unittest.TestCase):
    def test_land_and_sea(self):
        with tempfile.TemporaryDirectory() as directory:
            path = str(Path(directory) / 'mask.tif')
            driver = gdal.GetDriverByName('GTiff')
            dataset = driver.Create(path, 4, 4, 1, gdal.GDT_Byte)
            dataset.SetGeoTransform((0, 1, 0, 4, 0, -1))
            cells = np.zeros((4, 4), dtype=np.uint8)
            cells[1, 1] = 1
            dataset.GetRasterBand(1).WriteArray(cells)
            dataset = None
            mask = LandMask(path)
            self.assertTrue(mask.has_land(0.5, 1.5, 2.5, 3.5))
            self.assertFalse(mask.has_land(2.5, 0.0, 3.5, 1.0))
            self.assertFalse(mask.has_land(10.0, 10.0, 11.0, 11.0))


class SquareMathTest(unittest.TestCase):
    def test_paris_holdout_square(self):
        self.assertEqual((square_of_longitude(2.35), square_of_latitude(48.85)), (259, 176))

    def test_bounds_roundtrip(self):
        west, south, east, north = square_bounds(259, 176)
        self.assertLess(west, 2.35)
        self.assertLess(2.35, east)
        self.assertLess(south, 48.85)
        self.assertLess(48.85, north)
        self.assertAlmostEqual(west, 259 / 512 * 360 - 180)
        self.assertAlmostEqual(latitude_of_square_edge(176), north)

    def test_latitude_monotone(self):
        edges = [latitude_of_square_edge(y) for y in (100, 200, 300)]
        self.assertTrue(edges[0] > edges[1] > edges[2])


class FranceGroupsTest(unittest.TestCase):
    def test_mainland_and_corsica(self):
        fetch_fr = load_hyphenated('fetch-fr-rgealti')
        self.assertEqual(fetch_fr.group_of(650000, 6870000), 'FR-RGEALTI')
        self.assertEqual(fetch_fr.group_of(1180000, 6100000), 'FR-RGEALTI-CORSE')
        self.assertEqual(fetch_fr.group_of(1040000, 6250000), 'FR-RGEALTI')


class SnapWindowTest(unittest.TestCase):
    def test_snaps_to_native_grid(self):
        crop = load_hyphenated('crop-gedtm')
        dataset = gdal.GetDriverByName('MEM').Create('', 360, 180, 1, gdal.GDT_Float32)
        dataset.SetGeoTransform((-180.0, 1.0, 0.0, 90.0, 0.0, -1.0))
        self.assertEqual(crop.snap_window(dataset, (2.2, 48.1, 2.9, 48.9)), (182, 41, 1, 1))


class PublishPathTest(unittest.TestCase):
    def test_roundtrip_and_refusal(self):
        from terrain_io import publish_path
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            staged = root / 'staged.bin'
            staged.write_bytes(b'x' * 100)
            target = root / 'sub' / 'final.bin'
            publish_path(target, staged)
            self.assertEqual(target.read_bytes(), b'x' * 100)
            self.assertFalse(staged.exists())
            staged.write_bytes(b'x' * 100)
            publish_path(target, staged)
            staged.write_bytes(b'y' * 100)
            with self.assertRaises(ValueError):
                publish_path(target, staged)


if __name__ == '__main__':
    unittest.main()
