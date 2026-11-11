"""National terrain fetches retry truncation, refuse server scaling and preserve native block means."""
import gzip
import hashlib
from http.client import IncompleteRead
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
import tempfile
from threading import Thread
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import numpy as np
from osgeo import gdal
from national_dem import average_tile, sheet, unscaled, wcs_window

FIXTURES = Path(__file__).parent / "fixtures"
FLEMISH = "https://geo.api.vlaanderen.be/DHMV/wcs"
CRS = "http://www.opengis.net/def/crs/EPSG/0/31370"


class NationalDemTests(unittest.TestCase):
    def test_interrupted_sheet_retries_without_publishing_partial_bytes(self):
        body = b"ncols 2\nnrows 2\nxllcorner 0\nyllcorner 0\ncellsize 1\nNODATA_value -9999\n1 2\n3 4\n"

        class InterruptedResponse(BaseHTTPRequestHandler):
            def do_GET(self):
                self.server.requests += 1
                self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.send_header("Content-Type", "text/plain")
                self.end_headers()
                self.wfile.write(body[:-3] if self.server.requests <= self.server.failures else body)

            def log_message(self, *_):
                pass

        for failures in (1, 4):
            with self.subTest(failures=failures), tempfile.TemporaryDirectory() as temporary, \
                    HTTPServer(("127.0.0.1", 0), InterruptedResponse) as server:
                server.requests, server.failures = 0, failures
                worker = Thread(target=server.serve_forever, args=(0.01,), daemon=True)
                worker.start()
                arguments = SimpleNamespace(raw=Path(temporary), reserve_bytes=0)
                row = dict(sheet="test", url=f"http://127.0.0.1:{server.server_port}/sheet.asc")
                try:
                    with patch("national_dem.time.sleep") as sleep:
                        if failures == 4:
                            with self.assertRaises(IncompleteRead):
                                sheet(arguments, row)
                            self.assertEqual(list(arguments.raw.iterdir()), [])
                        else:
                            record = sheet(arguments, row)
                            self.assertEqual(gzip.decompress(Path(record["path"]).read_bytes()), body)
                            self.assertEqual(record["sha256"], hashlib.sha256(body).hexdigest())
                            self.assertEqual(record["bytes"], len(body))
                            self.assertEqual(sheet(arguments, row), record)
                        attempts = min(failures + 1, 4)
                        self.assertEqual(server.requests, attempts)
                        self.assertEqual(sleep.call_count, attempts - 1)
                finally:
                    server.shutdown()
                    worker.join()

    def test_server_scaling_is_refused_for_every_wcs_scaling_parameter(self):
        # The two requests whose responses held heights absent from the publisher's native data.
        flemish = (f"{FLEMISH}?service=WCS&version=2.0.1&request=GetCoverage&CRS=EPSG:31370"
                   f"&subset=x,{CRS}(150000,160000)&subset=y,{CRS}(210000,220000)&SCALESIZE=x(2000),y(2000)"
                   "&COVERAGEID=DHMVII_DTM_1m&FORMAT=image/geotiff&RESPONSE_CRS=EPSG:31370")
        polish = ("https://mapy.geoportal.gov.pl/wss/service/PZGIK/NMT/GRID1/WCS/DigitalTerrainModel?SERVICE=WCS"
                  "&VERSION=2.0.1&REQUEST=GetCoverage&COVERAGEID=DTM_PL-EVRF2007-NH&FORMAT=image%2Fx-aaigrid"
                  "&SCALEFACTOR=0.1&SUBSET=x%28269210.0%2C275210.0%29&SUBSET=y%28350070.0%2C356070.0%29")
        for url in (flemish, polish, polish.replace("SCALEFACTOR", "scalefactor"),
                    flemish.replace("SCALESIZE=x(2000),y(2000)", "SCALEAXES=x(0.2),y(0.2)"),
                    flemish.replace("SCALESIZE=x(2000),y(2000)", "SCALEEXTENT=x(0,2000),y(0,2000)")):
            with self.assertRaisesRegex(ValueError, "server-side scaling"):
                unscaled(url)
        native = wcs_window(FLEMISH, "DHMVII_DTM_1m", 31370, 152110, 211420, 152170, 211480)
        self.assertEqual(unscaled(native), native)
        self.assertNotIn("SCALE", native.upper())

    def test_native_windows_average_to_their_block_means_where_the_server_invented_heights(self):
        # Antwerp: the server's 5 m response held 242-254 m over this ground at 7.1-8.4 m.
        # Lower Silesia: the server's 10 m response held 0.0 in four cells over ground at 321-325 m.
        for name, epsg, step, west, north in [("national-dem-antwerp.tif", 31370, 5, 152110, 211480),
                                              ("national-dem-lower-silesia.tif", 2180, 10, 270480, 350940)]:
            native = gdal.Open(str(FIXTURES / name)).ReadAsArray().astype(np.float64)
            cells = native.shape[0] // step
            expected = native.reshape(cells, step, cells, step).mean(axis=(1, 3))
            with tempfile.TemporaryDirectory() as temporary:
                target = Path(temporary) / "tile.tif"
                average_tile([FIXTURES / name], epsg, [west, north - cells * step, west + cells * step, north],
                             step, target)
                averaged = gdal.Open(str(target)).ReadAsArray()
            self.assertEqual(averaged.shape, expected.shape)
            np.testing.assert_allclose(averaged, expected, atol=1e-4)
            self.assertGreaterEqual(averaged.min(), native.min())
            self.assertLessEqual(averaged.max(), native.max())


if __name__ == "__main__":
    unittest.main()
