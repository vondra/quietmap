"""Offline contracts for the German Länder index parsers and window maths."""
import functools
import http.server
import importlib.util
import json
from pathlib import Path
import tempfile
import threading
import unittest
import zipfile

tiles = importlib.util.spec_from_file_location('fetch_de_tiles', 'fetch-de-tiles.py')
fetch_tiles = importlib.util.module_from_spec(tiles)
tiles.loader.exec_module(fetch_tiles)
wcs = importlib.util.spec_from_file_location('fetch_de_wcs', 'fetch-de-wcs.py')
fetch_wcs = importlib.util.module_from_spec(wcs)
wcs.loader.exec_module(fetch_wcs)
sh = importlib.util.spec_from_file_location('fetch_de_sh', 'fetch-de-sh.py')
fetch_sh = importlib.util.module_from_spec(sh)
sh.loader.exec_module(fetch_sh)
he = importlib.util.spec_from_file_location('fetch_de_he', 'fetch-de-he.py')
fetch_he = importlib.util.module_from_spec(he)
he.loader.exec_module(fetch_he)
singles = importlib.util.spec_from_file_location('fetch_de_singles', 'fetch-de-singles.py')
fetch_singles = importlib.util.module_from_spec(singles)
singles.loader.exec_module(fetch_singles)


class ParserTest(unittest.TestCase):
    def test_metalink_keeps_geotiffs_with_hashes(self):
        data = '''<?xml version="1.0"?><metalink xmlns="urn:ietf:params:xml:ns:metalink">
        <file name="dgm1_32_419_5490_1_rp_2022.tif"><size>1502657</size>
        <hash type="sha-256">abc</hash><url>https://example.invalid/a.tif</url></file>
        <file name="dgm1_32_419_5490_1_rp_2022.tfw"><size>90</size>
        <url>https://example.invalid/a.tfw</url></file></metalink>'''
        items = fetch_tiles.parse_metalink(data)
        self.assertEqual(len(items), 1)
        self.assertEqual(items[0]['expected_sha256'], 'abc')
        self.assertEqual(items[0]['expected_size'], 1502657)
        self.assertEqual(items[0]['epoch'], 'ALS 2022')

    def test_stac_page_reports_items_and_paging(self):
        page = {'features': [{'id': 'dgm1_32_597_5909_1_ni_2025',
                              'assets': {'dgm1-tif': {'href': 'https://example.invalid/t.tif'}},
                              'properties': {'datetime': '2025-03-09T00:00:00Z'}}],
                'links': [{'rel': 'next', 'href': 'https://example.invalid/next'}]}
        items, more = fetch_tiles.parse_stac_page(page)
        self.assertTrue(more)
        self.assertEqual(items[0]['name'], 'dgm1_32_597_5909_1_ni_2025.tif')
        self.assertEqual(items[0]['epoch'], 'ALS 2025-03-09')

    def test_arcgis_page_keeps_dgm1_and_transfer_flag(self):
        page = {'features': [
            {'attributes': {'Produkt': 'DGM1', 'Kachel': '2785590',
                            'Download': 'https://example.invalid/z.zip', 'Stand': '2021-04-26'}},
            {'attributes': {'Produkt': 'DOM1', 'Kachel': '2785590',
                            'Download': 'https://example.invalid/d.zip', 'Stand': '2021'}}],
            'exceededTransferLimit': True}
        items, more = fetch_tiles.parse_arcgis_page(page)
        self.assertTrue(more)
        self.assertEqual(len(items), 1)
        self.assertEqual(items[0]['name'], 'dgm1_sn_2785590.zip')

    def test_atom_prefers_the_newest_epoch_per_kachel(self):
        feed = '''<?xml version="1.0"?><feed xmlns="http://www.w3.org/2005/Atom">
        <entry><link rel="section" href="https://example.invalid/hoehendaten/DGM/dgm_2010-2013/dgm2_561_5609_1_th_2010-2013.zip"/></entry>
        <entry><link rel="section" href="https://example.invalid/hoehendaten/DGM/dgm_2020-2025/dgm1_32_561_5609_1_th_2020-2025.zip"/></entry>
        <entry><link rel="section" href="https://example.invalid/hoehendaten/DGM/dgm_2014-2019/dgm1_32_562_5609_1_th_2014-2019.zip"/></entry>
        </feed>'''
        items = {item['name']: item for item in fetch_tiles.parse_atom(feed)}
        self.assertEqual(set(items), {'dgm1_32_561_5609_1_th_2020-2025.zip',
                                     'dgm1_32_562_5609_1_th_2014-2019.zip'})
        self.assertEqual(items['dgm1_32_561_5609_1_th_2020-2025.zip']['epoch'], 'ALS 2020-2025')


class RangeHandler(http.server.SimpleHTTPRequestHandler):
    def send_head(self):
        if 'Range' not in self.headers:
            return super().send_head()
        path = self.translate_path(self.path)
        data = Path(path).read_bytes()
        first, last = self.headers['Range'].replace('bytes=', '').split('-')
        first, last = int(first or 0), int(last or len(data) - 1)
        body = data[first:last + 1]
        self.send_response(206)
        self.send_header('Content-Range', f'bytes {first}-{last}/{len(data)}')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        return body

    def do_GET(self):
        body = self.send_head()
        if body is not None:
            self.wfile.write(body if isinstance(body, bytes) else body.read())

    def log_message(self, *args):
        pass


class ZipMemberTest(unittest.TestCase):
    def test_range_read_returns_the_named_member_bytes(self):
        with tempfile.TemporaryDirectory() as temp:
            archive = Path(temp) / 'tile.zip'
            with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED) as handle:
                handle.writestr('tile.xyz', 'x' * 10000)
                handle.writestr('tile.tif', 'tif-bytes')
            server = http.server.ThreadingHTTPServer(
                ('127.0.0.1', 0), functools.partial(RangeHandler, directory=temp))
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                url = f'http://127.0.0.1:{server.server_port}/tile.zip'
                identity, raw = fetch_tiles.fetch_zip_member(url, '.tif')
            finally:
                server.shutdown()
                thread.join()
            self.assertEqual(raw, b'tif-bytes')
            self.assertEqual(identity['member'], 'tile.tif')
            self.assertEqual(identity['file_bytes'], archive.stat().st_size)


class IndexTest(unittest.TestCase):
    def test_sh_index_keeps_tile_links_and_dates(self):
        import json as json_module
        document = json_module.dumps({'features': [
            {'properties': {'kachel': '324246002', 'datum': '2005-03-09',
                            'link_data': 'https://example.invalid/massen.php?file=tile_xyz.xyz&id=2'}},
            {'properties': {'kachel': '324246003', 'datum': '2024-05-01',
                            'link_data': 'https://example.invalid/massen.php?file=other.xyz&id=2'}}]})
        items = {item['name']: item for item in fetch_sh.parse_index(document)}
        self.assertEqual(set(items), {'tile_xyz.xyz', 'other.xyz'})
        self.assertEqual(items['tile_xyz.xyz']['epoch'], 'ALS 2005-03-09')
        self.assertEqual(items['tile_xyz.xyz']['kachel'], '324246002')

    def test_sh_footer_cut_refuses_a_truncated_response(self):
        with tempfile.TemporaryDirectory() as temp:
            good = Path(temp) / 'tile.xyz'
            good.write_bytes(b'424000.50 6002999.50 -1.07\n<!DOCTYPE html><html></html>')
            cleaned = fetch_sh.strip_footer(good)
            self.assertEqual(cleaned.read_bytes(), b'424000.50 6002999.50 -1.07\n')
            bad = Path(temp) / 'short.xyz'
            bad.write_bytes(b'424000.50 6002999.50 -1.07\n')
            with self.assertRaises(ValueError):
                fetch_sh.strip_footer(bad)

    def test_he_kachel_key_matches_the_metadata_table(self):
        self.assertEqual(fetch_he.kachel_of('dgm1_32_492_5509_1_he.tif'), '4925509')
        self.assertEqual(fetch_he.excel_date(43803), '2019-12-04')

    def test_st_meta_and_hh_table_yield_per_tile_epochs(self):
        meta = 'Kachelname: dgm5_32_606_5760_2_st\r\nAktualitaet: 2019-03\r\n'
        self.assertEqual(fetch_singles.st_epoch(meta), 'ALS 2019-03')
        csv = ('Kachelinformationen des DGM1;;;;\r\n'
               'Kachelname;Aktualitaet;Erfassungsmethode\r\n'
               'dgm1_32_548_5934_1_hh_2022;2022-03;5020\r\n')
        epochs = fetch_singles.hh_epochs(csv)
        self.assertEqual(epochs['dgm1_32_548_5934_1_hh_2022'], 'ALS 2022-03')


class WindowTest(unittest.TestCase):
    def test_windows_tile_the_extent_without_gaps(self):
        boxes = list(fetch_wcs.windows((0.0, 0.0, 25000.0, 15000.0)))
        self.assertEqual(len(boxes), 6)
        coverage = sum((east - west) * (north - south) for west, south, east, north in boxes)
        self.assertAlmostEqual(coverage, 25000.0 * 15000.0)

    def test_coverage_url_scales_to_5m(self):
        url = fetch_wcs.coverage_url('de-nw-dgm1', 400000.0, 5700000.0, 410000.0, 5710000.0)
        self.assertIn('COVERAGEID=nw_dgm', url)
        self.assertIn('SUBSET=x(400000.0,410000.0)', url)
        self.assertIn('SCALESIZE=x(2000),y(2000)', url)
        bw = fetch_wcs.coverage_url('de-bw-dgm1', 500000.0, 5400000.0, 510000.0, 5410000.0)
        self.assertIn('SUBSET=E(500000.0,510000.0)', bw)
        self.assertIn('SCALESIZE=X(2000),Y(2000)', bw)


if __name__ == '__main__':
    unittest.main()
