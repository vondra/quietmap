"""GFW download plan: 365-day window, world tiles, class filters, resumable receipts."""

from datetime import datetime, timezone
import json
from pathlib import Path
import sqlite3
import sys
import tempfile
import unittest
import urllib.parse
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import download_gfw  # noqa: E402


class DownloadGfwTests(unittest.TestCase):
    def test_window_tiles_and_filters(self):
        tiles = list(download_gfw.tiles())
        self.assertEqual(len(tiles), 45 * 18)
        self.assertEqual(tiles[0], (-180, -60))
        self.assertEqual(download_gfw.tile_name(-180, -60), "lon-180_lat-60")
        ring = download_gfw.polygon(176, 80)["geojson"]["coordinates"][0]
        self.assertEqual(ring[2], [184, 84], "the last row stops at 84 N")
        url = download_gfw.report_url("work", "2025-09-01", "2026-08-31")
        query = urllib.parse.parse_qs(urllib.parse.urlsplit(url).query)
        self.assertEqual(query["filters[0]"], ["vessel_type in ('fishing','support','gear','seismic_vessel','other')"])
        self.assertEqual(query["date-range"], ["2025-09-01,2026-08-31"])
        self.assertEqual(query["format"], ["TIF"])

    def test_window_json_is_the_aircraft_exposure_year(self):
        cases = [("2027-01", datetime(2027, 1, 1, tzinfo=timezone.utc), ("2026-01-01", "2026-12-31", 365)),
                 ("2026-10", datetime(2026, 10, 1, tzinfo=timezone.utc), ("2025-10-01", "2026-09-30", 365)),
                 ("2024-03", datetime(2026, 9, 24, tzinfo=timezone.utc), ("2023-03-01", "2024-02-29", 366))]
        for anchor, now, expected in cases:
            with self.subTest(anchor=anchor), tempfile.TemporaryDirectory() as directory:
                output = Path(directory)
                (output / "token").write_text("t")
                with patch.object(download_gfw, "datetime") as clock, \
                        patch.object(download_gfw, "download") as download, \
                        patch("sys.argv", ["download_gfw.py", "--output", str(output), "--anchor", anchor,
                                           "--token-file", str(output / "token")]):
                    clock.now.return_value = now
                    download_gfw.main()
                window = json.loads((output / "window.json").read_text())
                self.assertEqual((window["first_day"], window["last_day"], window["days"]), expected)
                self.assertEqual(download.call_args.args[2:4], expected[:2])

    def test_download_resumes_from_receipts_and_stores_empty_tiles(self):
        class FakeClient:
            calls = []

            def report(self, url, body):
                FakeClient.calls.append(url)
                return b"" if "%27fishing%27" in url else b"zipbytes"  # the API host itself contains "fishing"
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            receipts = download_gfw.open_receipts(output / "receipts.sqlite")
            receipts.execute("INSERT INTO reports VALUES(?,?,?,?,?)", ("lon-180_lat-60-large", 1, "x", 0.1, "t"))
            receipts.commit()
            messages = []
            download_gfw.download(FakeClient(), output, "2025-09-01", "2026-08-31", receipts, messages.append)
            rows = receipts.execute("SELECT COUNT(*) FROM reports").fetchone()[0]
            self.assertEqual(rows, 45 * 18 * 2)
            self.assertEqual(len(FakeClient.calls), 45 * 18 * 2 - 1, "the receipted report is not fetched again")
            self.assertEqual((output / "lon-180_lat-60-work.zip").read_bytes(), b"")
            self.assertEqual((output / "lon+004_lat+04-large.zip").read_bytes(), b"zipbytes")
            self.assertIn("1619 to fetch", messages[0])


if __name__ == "__main__":
    unittest.main()
