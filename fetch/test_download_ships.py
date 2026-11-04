"""Verify complete API windows, report identity and resumption without making network requests."""
import contextlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import urllib.parse
import zipfile

import download_ships as downloader


def report(url, body, end="2001-01-01"):
    query = urllib.parse.parse_qs(urllib.parse.urlsplit(url).query)
    metadata = (f"Filters: {query['filters[0]'][0]} and "
                "timestamp >= parseDateTimeBestEffort('2000-01-01') and "
                f"timestamp < parseDateTimeBestEffort('{end}')")
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w") as archive:
        prefix = "readme_" if query["format"][0] == "CSV" else ""
        archive.writestr(f"layer-activity-data-0/{prefix}test-v4.0.md", metadata)
        archive.writestr("layer-reference-geometry/geometry.geojson",
                         json.dumps({"features": [{"geometry": body["geojson"]}]}))
    return output.getvalue()


class DownloadTests(unittest.TestCase):
    def test_main_fetches_the_last_day_resumes_exact_objects_and_rejects_modified_inputs(self):
        with tempfile.TemporaryDirectory(prefix="qm-gfw-download-") as directory:
            root = Path(directory)
            manifest = {"year": "2000", "flows": {"start": "2000-01-01", "end": "2001-01-01"}, "inputs": [{
                "id": "presence", "kind": "prepare-ships", "observed": {"start": "2000-01-01", "end": "2000-12-31"},
                "conversion": {"format": "presence-reports", "dataset": "test:v4.0", "window": "window.json",
                               "inventory": {"path": "inventory.json"},
                               "acquisition": {"bounds": [0, 0, 8, 8], "tile_degrees": 8,
                                               "classes": {"large": ["cargo"], "work": ["tug"]}}}}]}
            (root / "manifest.json").write_text(json.dumps(manifest))
            (root / "token").write_text("test-token")
            arguments = ["download_ships.py", "--year", str(root / "manifest.json"), "--sources", str(root),
                         "--input-id", "presence", "--token-file", str(root / "token")]
            requests = []

            def request(_client, url, body=None):
                requests.append(url)
                return 200, report(url, body)

            with patch("sys.argv", arguments), patch.object(downloader.Client, "request", request), contextlib.redirect_stdout(io.StringIO()):
                downloader.main()
                self.assertEqual(len(requests), 2)
                for url in requests:
                    query = urllib.parse.parse_qs(urllib.parse.urlsplit(url).query)
                    self.assertEqual(query["date-range"], ["2000-01-01,2001-01-01"])
                output = root / "2000/presence"
                before = (output / "inventory.json").read_bytes()
                downloader.main()
                self.assertEqual(len(requests), 2)
                self.assertEqual(before, (output / "inventory.json").read_bytes())
                with (output / "lon+000_lat+00-large.zip").open("ab") as stream:
                    stream.write(b"changed")
                with self.assertRaisesRegex(ValueError, "differs from its receipt"):
                    downloader.main()

    def test_publisher_metadata_rejects_a_missing_final_day_or_another_tile(self):
        url = ("https://example.org/report?format=TIF&datasets%5B0%5D=test%3Av4.0&"
               "date-range=2000-01-01%2C2001-01-01&filters%5B0%5D=vessel_type%20in%20%28%27cargo%27%29")
        body = {"geojson": {"type": "Polygon", "coordinates": [[[0, 0], [8, 0], [8, 8], [0, 8], [0, 0]]]}}
        downloader.validate_report(report(url, body), url, body)
        with self.assertRaisesRegex(ValueError, "window"):
            downloader.validate_report(report(url, body, "2000-12-31"), url, body)
        with self.assertRaisesRegex(ValueError, "geometry"):
            downloader.validate_report(report(url, body), url, {"geojson": {"type": "Polygon", "coordinates": []}})

    def test_sparse_tiff_falls_back_to_grouped_csv_with_the_same_query(self):
        url = ("https://example.org/report?format=TIF&datasets%5B0%5D=test%3Av4.0&"
               "date-range=2000-01-01%2C2001-01-01&filters%5B0%5D=vessel_type%20in%20%28%27cargo%27%29")
        body = {"geojson": {"type": "Polygon", "coordinates": [[[0, 0], [8, 0], [8, 8], [0, 8], [0, 0]]]}}
        csv_url = url.replace("format=TIF", "format=CSV") + "&group-by=FLAG"
        payload = report(csv_url, body)
        responses = [(503, b"exit status 1"), (503, b"exit status 1"), (200, payload)]
        client = downloader.Client("test-token")
        with patch.object(client, "request", side_effect=responses) as request, patch.object(downloader.time, "sleep"):
            self.assertEqual(client.report(url, body), payload)
        self.assertEqual(request.call_args_list[-1].args, (csv_url, body))


if __name__ == "__main__":
    unittest.main()
