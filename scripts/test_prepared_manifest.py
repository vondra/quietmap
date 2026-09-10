"""Prepared generation pins are deterministic and never publish a partial replacement."""

import hashlib
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch

import prepared_manifest


class PreparedManifestTests(unittest.TestCase):
    def test_manifest_pins_arrow_content_in_canonical_path_order(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            square = root / 'prepared/z9/276/173'
            square.mkdir(parents=True)
            (square / 'roads.arrow').write_bytes(b'roads')
            (square / 'structures.arrow').write_bytes(b'structures')
            (square / 'roads.arrow.tmp').write_bytes(b'incomplete')
            first, second = root / 'a.sqlite', root / 'b.sqlite'
            a = prepared_manifest.write_manifest(root / 'prepared', first)
            b = prepared_manifest.write_manifest(root / 'prepared', second)
            self.assertEqual(a, b)
            self.assertEqual(first.read_bytes(), second.read_bytes())
            with sqlite3.connect(first) as database:
                rows = database.execute('SELECT relative_path,sha256 FROM input_files ORDER BY relative_path').fetchall()
            self.assertEqual(rows, [('z9/276/173/roads.arrow', hashlib.sha256(b'roads').digest()),
                                    ('z9/276/173/structures.arrow', hashlib.sha256(b'structures').digest())])
            self.assertEqual(a['files'], 2)
            (root / 'prepared/z9/junk').mkdir()
            previous = first.read_bytes()
            with self.assertRaisesRegex(ValueError, 'canonical square column'):
                prepared_manifest.write_manifest(root / 'prepared', first)
            self.assertEqual(first.read_bytes(), previous)

    def test_mutating_input_preserves_the_previous_published_manifest(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            square = root / 'prepared/z9/276/173'
            square.mkdir(parents=True)
            source = square / 'roads.arrow'
            source.write_bytes(b'roads')
            output = root / 'manifest.sqlite'
            prepared_manifest.write_manifest(root / 'prepared', output)
            previous = output.read_bytes()
            original = prepared_manifest.sha256

            def changed(path):
                digest = original(path)
                if path == source:
                    path.write_bytes(b'changed source')
                return digest

            with patch.object(prepared_manifest, 'sha256', side_effect=changed):
                with self.assertRaisesRegex(ValueError, 'changed while hashing'):
                    prepared_manifest.write_manifest(root / 'prepared', output)
            self.assertEqual(output.read_bytes(), previous)
            self.assertEqual(sorted(path.name for path in root.iterdir()), ['manifest.sqlite', 'prepared'])


if __name__ == '__main__':
    unittest.main()
