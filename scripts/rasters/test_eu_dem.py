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


class DownloadRetryTest(unittest.TestCase):
    def test_retries_transient_400_then_succeeds(self):
        import urllib.request
        from dem_windows import download_bytes
        real = urllib.request.urlopen
        calls = []

        class FakeResponse:
            headers = {}
            def read(self):
                return b'tiff-bytes'

            def __enter__(self):
                return self

            def __exit__(self, *args):
                return False

        def failing_twice(request, timeout=None):
            calls.append(request)
            if len(calls) < 3:
                raise urllib.error.HTTPError(request.full_url, 400, 'Bad Request', {}, None)
            return FakeResponse()

        urllib.request.urlopen = failing_twice
        try:
            body, _ = download_bytes('https://example.invalid/wms')
        finally:
            urllib.request.urlopen = real
        self.assertEqual(body, b'tiff-bytes')
        self.assertEqual(len(calls), 3)

    def test_other_client_errors_fail_fast(self):
        import urllib.request
        from dem_windows import download_bytes
        real = urllib.request.urlopen
        calls = []

        def forbidden(request, timeout=None):
            calls.append(request)
            raise urllib.error.HTTPError(request.full_url, 403, 'Forbidden', {}, None)

        urllib.request.urlopen = forbidden
        try:
            with self.assertRaises(urllib.error.HTTPError):
                download_bytes('https://example.invalid/wms')
        finally:
            urllib.request.urlopen = real
        self.assertEqual(len(calls), 1)


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


class CHDedupeTest(unittest.TestCase):
    def item(self, year, tile):
        return {'id': f'swissalti3d_{year}_{tile}',
                'properties': {'datetime': f'{year}-01-01T00:00:00Z'}}

    def test_keeps_newest_item_per_tile(self):
        fetch_ch = load_hyphenated('fetch-ch-alti3d')
        items = [self.item('2019', '2485-1109'), self.item('2025', '2485-1109'),
                 self.item('2019', '2486-1109')]
        kept = fetch_ch.latest_per_tile(items)
        self.assertEqual([item['id'] for item in kept],
                         ['swissalti3d_2019_2486-1109', 'swissalti3d_2025_2485-1109'])

    def test_rejects_unexpected_tile_id(self):
        fetch_ch = load_hyphenated('fetch-ch-alti3d')
        with self.assertRaisesRegex(ValueError, 'unexpected swissALTI3D tile id'):
            fetch_ch.latest_per_tile([{'id': 'swissalti3d_2019_national',
                                       'properties': {'datetime': '2019-01-01T00:00:00Z'}}])


class CHFetchTest(unittest.TestCase):
    ITEM = {'id': 'swissalti3d_2019_2485-1109',
            'assets': {'swissalti3d_2019_2485-1109_2_2056_5728.tif':
                       {'href': 'https://example.invalid/tile'}}}

    def payload(self):
        mem = gdal.GetDriverByName('MEM').Create('', 500, 500, 1, gdal.GDT_Float32)
        mem.SetGeoTransform((2485000, 2, 0, 1110000, 0, -2))
        mem.GetRasterBand(1).SetNoDataValue(-9999)
        mem.GetRasterBand(1).WriteArray(np.full((500, 500), 412.5, dtype=np.float32))
        with tempfile.TemporaryDirectory() as directory:
            path = str(Path(directory) / 'tile.tif')
            gdal.Translate(path, mem, format='GTiff')
            return Path(path).read_bytes()

    def test_fetches_tile_with_verified_receipt(self):
        fetch_ch = load_hyphenated('fetch-ch-alti3d')
        calls = []
        payload = self.payload()

        def fake_download(url):
            calls.append(url)
            return payload, {}

        fetch_ch.download_bytes = fake_download
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            record = fetch_ch.fetch_item(root, self.ITEM, 1, 1)
            target = root / fetch_ch.PROVIDER / 'swissalti3d_2019_2485-1109_2_2056_5728.tif'
            self.assertEqual(record['url'], 'https://example.invalid/tile')
            self.assertTrue(record['raw_bytes_retained'])
            self.assertEqual(record['bytes'], target.stat().st_size)
            dataset = gdal.Open(str(target))
            self.assertEqual(dataset.GetRasterBand(1).ReadAsArray()[0, 0], 412.5)
            resumed = fetch_ch.fetch_item(root, self.ITEM, 1, 1)
            self.assertEqual(resumed, record)
            self.assertEqual(len(calls), 1)

    def test_rejects_tile_of_another_parent(self):
        fetch_ch = load_hyphenated('fetch-ch-alti3d')
        fetch_ch.download_bytes = lambda url: (self.payload(), {})
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fetch_ch.fetch_item(root, self.ITEM, 1, 1)
            other = {'id': self.ITEM['id'],
                     'assets': {'swissalti3d_2019_2485-1109_2_2056_5728.tif':
                                {'href': 'https://example.invalid/elsewhere'}}}
            with self.assertRaisesRegex(ValueError, 'another parent'):
                fetch_ch.fetch_item(root, other, 1, 1)


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


class PTRetainedTileTest(unittest.TestCase):
    ITEM = 'MDT-2m-110208-07-2024'

    def write_tile(self, root, origin_x, receipt_extra=None):
        from terrain_io import digest
        target = root / 'pt-dgt' / (self.ITEM + '.tif')
        target.parent.mkdir(parents=True, exist_ok=True)
        dataset = gdal.GetDriverByName('GTiff').Create(str(target), 4, 4, 1, gdal.GDT_Float32)
        dataset.SetGeoTransform((origin_x, 2, 0, -92000, 0, -2))
        dataset.GetRasterBand(1).SetNoDataValue(-999)
        dataset.GetRasterBand(1).WriteArray(np.full((4, 4), 12.5, dtype=np.float32))
        dataset = None
        record = dict(url='https://example.invalid/search', fetched_utc='2026-09-25T00:00:00+00:00',
                      sha256=digest(target), bytes=target.stat().st_size, licence='CC BY 4.0',
                      licence_url='https://creativecommons.org/licenses/by/4.0/',
                      terms_checked_utc='2026-09-25', **(receipt_extra or {}))
        Path(str(target) + '.provenance.json').write_text(json.dumps(record))
        return target

    def item(self):
        return {'id': self.ITEM, 'assets': {'data': {'href': 'https://example.invalid/tile'}}}

    def test_adopts_earlier_tile_without_item_id(self):
        fetch_pt = load_hyphenated('fetch-pt-dgt')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.write_tile(root, -90000)
            _, record, error = fetch_pt.fetch_tile(root, self.item(), None, 1, 1)
            self.assertIsNone(error)
            self.assertNotIn('item_id', record)

    def test_rejects_tile_of_another_item(self):
        fetch_pt = load_hyphenated('fetch-pt-dgt')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.write_tile(root, -90000, {'item_id': 'MDT-2m-999999-07-2024'})
            with self.assertRaisesRegex(ValueError, 'another item'):
                fetch_pt.fetch_tile(root, self.item(), None, 1, 1)

    def test_off_grid_earlier_tile_needs_review(self):
        fetch_pt = load_hyphenated('fetch-pt-dgt')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            before = self.write_tile(root, -89999).read_bytes()
            with self.assertRaisesRegex(ValueError, 'off the national grid'):
                fetch_pt.fetch_tile(root, self.item(), None, 1, 1)
            self.assertEqual((root / 'pt-dgt' / (self.ITEM + '.tif')).read_bytes(), before)


class DKZeroRemapTest(unittest.TestCase):
    WINDOW = (600000, 6220000, 601000, 6221000)

    def payload(self, values):
        fetch_dk = load_hyphenated('fetch-dk-dhm')
        rows, columns = values.shape
        mem = gdal.GetDriverByName('MEM').Create('', columns, rows, 1, gdal.GDT_Float32)
        mem.SetGeoTransform((self.WINDOW[0], fetch_dk.RESOLUTION, 0, self.WINDOW[3],
                             0, -fetch_dk.RESOLUTION))
        mem.GetRasterBand(1).WriteArray(values)
        with tempfile.TemporaryDirectory() as directory:
            path = str(Path(directory) / 'wcs.tif')
            gdal.Translate(path, mem, format='GTiff')
            return Path(path).read_bytes()

    def run_window(self, payload):
        fetch_dk = load_hyphenated('fetch-dk-dhm')
        fetch_dk.download_bytes = lambda url: (payload, {})
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            window, record = fetch_dk.fetch_window(root, self.WINDOW, None, 'token', 1, 1)
            target = root / fetch_dk.PROVIDER / 'dhm_terraen_25m_600000_6220000.tif'
            if record is None:
                return window, record, None, None, target.exists()
            dataset = gdal.Open(str(target))
            nodata = dataset.GetRasterBand(1).GetNoDataValue()
            return window, record, nodata, dataset.GetRasterBand(1).ReadAsArray(), True

    def test_exact_zeros_become_nodata(self):
        values = np.full((40, 40), 7.5, dtype=np.float32)
        values[0, 0] = 0
        window, record, nodata, kept, _ = self.run_window(self.payload(values))
        self.assertEqual(window, self.WINDOW)
        self.assertIsNotNone(record)
        self.assertEqual(nodata, -9999)
        self.assertEqual(kept[0, 0], -9999)
        self.assertEqual(kept[1, 1], 7.5)
        self.assertNotIn(0, kept)

    def test_all_zero_sea_window_is_skipped(self):
        window, record, _, _, exists = self.run_window(
            self.payload(np.zeros((40, 40), dtype=np.float32)))
        self.assertEqual(window, self.WINDOW)
        self.assertIsNone(record)
        self.assertFalse(exists)


if __name__ == '__main__':
    unittest.main()
