"""fetch() provenance and amortized budget enforcement."""
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import terrain_io
from terrain_io import fetch


class FetchTest(unittest.TestCase):
    def test_fetch_writes_provenance_with_and_without_budget_check(self):
        with tempfile.TemporaryDirectory() as temp:
            payload = Path(temp) / 'payload.bin'
            payload.write_bytes(b'x' * 1000)
            url = payload.as_uri()
            root = Path(temp) / 'root'
            for name, enforce in (('a.bin', True), ('b.bin', False)):
                record = fetch(root, 'prov', name, url, licence='L', licence_url='U',
                               terms_checked_utc='2026-09-25', enforce_budget=enforce)
                self.assertEqual(record['bytes'], 1000)
                self.assertEqual(record['url'], url)
                self.assertTrue((root / 'prov' / (name + '.provenance.json')).exists())
            again = fetch(root, 'prov', 'a.bin', url, licence='L', licence_url='U',
                          terms_checked_utc='2026-09-25')
            self.assertEqual(again['bytes'], 1000)

    def test_budget_refusal_only_applies_on_enforced_calls(self):
        with tempfile.TemporaryDirectory() as temp:
            payload = Path(temp) / 'payload.bin'
            payload.write_bytes(b'x' * 1000)
            url = payload.as_uri()
            root = Path(temp) / 'root'
            old = terrain_io.MAX_DOWNLOAD_BYTES
            terrain_io.MAX_DOWNLOAD_BYTES = 10
            try:
                with self.assertRaises(ValueError):
                    fetch(root, 'prov', 'a.bin', url, licence='L', licence_url='U',
                          terms_checked_utc='2026-09-25', enforce_budget=True)
                record = fetch(root, 'prov', 'b.bin', url, licence='L', licence_url='U',
                               terms_checked_utc='2026-09-25', enforce_budget=False)
                self.assertEqual(record['bytes'], 1000)
            finally:
                terrain_io.MAX_DOWNLOAD_BYTES = old


if __name__ == '__main__':
    unittest.main()
