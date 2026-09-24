"""The input freeze stores one SHA256SUMS per family, checks every stored checksum and never writes into inputs."""

import contextlib
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import sqlite3
import sys
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('freeze', Path(__file__).with_name('freeze-world-inputs.py'))
freeze = importlib.util.module_from_spec(spec)
spec.loader.exec_module(freeze)


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def tree_snapshot(root):
    return {str(path): path.read_bytes() for path in sorted(root.rglob('*')) if path.is_file()}


class FreezeWorldInputsTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        sources = self.root / 'sources'
        self.planet = sources / 'osm/planet.pbf'
        self.planet.parent.mkdir(parents=True)
        self.planet.write_bytes(b'planet')
        self.emodnet = sources / 'emodnet'
        self.emodnet.mkdir()
        (self.emodnet / 'density.tif').write_bytes(b'density')
        (self.emodnet / 'sha256.txt').write_text(f'{sha256(b"density")}  density.tif\n')
        self.gfw = sources / 'gfw'
        self.gfw.mkdir()
        (self.gfw / 'tile-large.zip').write_bytes(b'zip')
        with contextlib.closing(sqlite3.connect(self.gfw / 'receipts.sqlite')) as database, database:
            database.execute('CREATE TABLE reports(name TEXT, sha256 TEXT)')
            database.execute('INSERT INTO reports VALUES (?, ?)', ('tile-large', sha256(b'zip')))
        self.ga = sources / 'adsblol'
        part = self.ga / '2026/2026-06-06/v2026.06.06-planes-readsb-prod-0.tar.aa'
        part.parent.mkdir(parents=True)
        part.write_bytes(b'part')
        with contextlib.closing(sqlite3.connect(self.ga / 'catalog.sqlite')) as database, database:
            database.execute('CREATE TABLE assets(name TEXT, sha256 TEXT)')
            database.execute('CREATE TABLE verified(path TEXT, sha256 TEXT)')
            database.execute('INSERT INTO assets VALUES (?, ?)', (part.name, sha256(b'part')))
            # A recorded directory from before a disk migration still names the same file.
            database.execute('INSERT INTO verified VALUES (?, ?)', (f'/retired-disk/adsblol/2026/2026-06-06/{part.name}', sha256(b'part')))
            database.execute('INSERT INTO assets VALUES (?, ?)', ('replaced-by-an-alternative.tar', sha256(b'gone')))
        self.part = part
        self.sources = {'planet': self.planet, 'ships': self.emodnet, 'ships_gfw': self.gfw, 'general_aviation': self.ga}
        self.output = self.root / 'freeze'
        # The TOML's [sources] contract belongs to source_paths; these tests stub it with four families.
        self.config = self.root / 'build.toml'
        self.config.write_text('[sources]\n')

    def run_freeze(self, output=None):
        argv = ['freeze-world-inputs.py', '--config', str(self.config), '--output', str(output or self.output)]
        stdout, stderr = io.StringIO(), io.StringIO()
        with patch.object(sys, 'argv', argv), patch.object(freeze, 'source_paths', return_value=self.sources), \
                contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            try:
                freeze.main()
                code = 0
            except SystemExit as exit_:
                code = exit_.code
        return code, stdout.getvalue(), stderr.getvalue()

    def test_first_run_writes_family_sums_and_checks_every_stored_record_without_touching_inputs(self):
        before = tree_snapshot(self.root / 'sources')
        code, stdout, stderr = self.run_freeze()
        self.assertEqual(code, 0, stderr)
        receipt = json.loads(stdout)
        self.assertEqual({family: row['stored_checksums_checked'] for family, row in receipt.items()},
                         {'general_aviation': 2, 'planet': 0, 'ships': 1, 'ships_gfw': 1})
        self.assertEqual((self.output / 'planet.SHA256SUMS').read_text(), f'{sha256(b"planet")}  planet.pbf\n')
        sums = (self.output / 'general_aviation.SHA256SUMS').read_text()
        self.assertIn(f'{sha256(b"part")}  2026/2026-06-06/{self.part.name}\n', sums)
        self.assertEqual(receipt['general_aviation']['sha256sums_sha256'], sha256(sums.encode()))
        self.assertEqual(tree_snapshot(self.root / 'sources'), before)

        written = (self.output / 'planet.SHA256SUMS').stat().st_mtime_ns
        code, stdout, stderr = self.run_freeze()
        self.assertEqual(code, 0, stderr)
        self.assertEqual(json.loads(stdout), receipt)
        self.assertEqual((self.output / 'planet.SHA256SUMS').stat().st_mtime_ns, written)

        self.planet.write_bytes(b'planet, edited after the freeze')
        code, _, stderr = self.run_freeze()
        self.assertIn('planet.pbf: differs from the frozen', stderr)
        self.assertIn('1 checksum failures', str(code))

    def test_a_stored_checksum_mismatch_fails_and_that_family_gets_no_sums(self):
        self.part.write_bytes(b'corrupt')
        (self.emodnet / 'density.tif').unlink()
        code, _, stderr = self.run_freeze()
        self.assertIn('3 checksum failures', str(code))
        self.assertIn('catalog.sqlite:assets expects', stderr)
        self.assertIn('catalog.sqlite:verified expects', stderr)
        self.assertIn('density.tif: listed by', stderr)
        self.assertFalse((self.output / 'general_aviation.SHA256SUMS').exists())
        self.assertFalse((self.output / 'ships.SHA256SUMS').exists())
        self.assertTrue((self.output / 'ships_gfw.SHA256SUMS').exists())

    def test_output_inside_a_source_is_refused(self):
        code, _, stderr = self.run_freeze(self.gfw / 'freeze')
        self.assertEqual(code, 2)
        self.assertIn('the freeze never writes into inputs', stderr)
        self.assertFalse((self.gfw / 'freeze').exists())


if __name__ == '__main__':
    unittest.main()
