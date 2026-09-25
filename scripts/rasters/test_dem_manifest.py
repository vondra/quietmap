"""Manifest assembly keeps provider groups contiguous and complete."""
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import dem_manifest


def listing(root, provider, entries):
    path = Path(root) / provider / 'country-sources.json'
    path.parent.mkdir(parents=True, exist_ok=True)
    import json
    path.write_text(json.dumps(entries))
    for entry in entries:
        Path(entry['path']).write_text('grid')
        Path(entry['path'] + '.provenance.json').write_text('{}')


class ManifestTest(unittest.TestCase):
    def test_groups_stay_contiguous_in_precedence_order(self):
        with tempfile.TemporaryDirectory() as temp:
            fb = [dict(path=str(Path(temp) / 'fb.tif'), role='fallback', group='FB')]
            na = [dict(path=str(Path(temp) / 'n1.tif'), role='national', group='DE-X'),
                  dict(path=str(Path(temp) / 'n2.tif'), role='national', group='DE-X')]
            listing(temp, 'fb', fb)
            listing(temp, 'de-x', na)
            sources = dem_manifest.load_sources(temp, ['fb', 'de-x'])
            self.assertEqual([s['group'] for s in sources], ['FB', 'DE-X', 'DE-X'])
            listing(temp, 'split', na[:1] + fb[:1])
            with self.assertRaises(ValueError):
                dem_manifest.load_sources(temp, ['split'])
            listing(temp, 'empty', [])
            with self.assertRaises(ValueError):
                dem_manifest.load_sources(temp, ['empty'])

    def test_missing_files_and_provenance_are_refused(self):
        with tempfile.TemporaryDirectory() as temp:
            listing(temp, 'de-x', [dict(path=str(Path(temp) / 'n1.tif'),
                                             role='national', group='DE-X')])
            Path(temp, 'n1.tif').unlink()
            with self.assertRaises(ValueError):
                dem_manifest.load_sources(temp, ['de-x'])


if __name__ == '__main__':
    unittest.main()
