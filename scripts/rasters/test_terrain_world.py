"""Prevent resumable production from escaping its source budget or losing launch context."""
import importlib.util
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import zipfile
import terrain_io
import terrain_world
import terrain_produce
import numpy as np


class WorldTest(unittest.TestCase):
    def test_launch_preserves_relative_paths_and_releases_failed_units(self):
        args = ['terrain_world.py', 'launch', '--coverage', 'coverage.json.gz', '--dem', 'dem.json',
                '--canopy', 'canopy.json', '--source-root', 'sources', '--output', 'output',
                '--raster-repack', 'raster-repack', '--coverage-sha256', 'abc', '--reserve-bytes', '0']
        with patch('sys.argv', args), patch.object(terrain_world, 'preflight'), \
                patch.object(terrain_world.subprocess, 'run') as launch:
            terrain_world.main()
        command = launch.call_args.args[0]
        self.assertIn('--same-dir', command)
        self.assertIn('--collect', command)
        self.assertIn('CPUQuota=800%', command)

    def test_preflight_rejects_missing_or_unaccounted_source_roots(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); sources = root / 'sources'; sources.mkdir()
            binary = root / 'binary'; binary.touch()
            other = root / 'outside.tif'; other.write_bytes(b'x')
            args = SimpleNamespace(coverage=root/'coverage', coverage_sha256='test', source_root=sources,
                                   raster_repack=binary, output=root/'out', reserve_bytes=0,
                                   dem=root/'dem.json', canopy=root/'canopy.json')
            for channel in ('dem', 'canopy'):
                (root/f'{channel}.json').write_text(json.dumps(dict(channel=channel, squares=[[0, 0]],
                                                       sources=[dict(path=str(other))])))
            coverage = dict(squares=[dict(x=0, y=0, rows=1, columns=1, status='land')])
            with patch.object(terrain_world, 'world_plan', return_value=coverage):
                with self.assertRaisesRegex(ValueError, 'outside the combined source-budget'):
                    terrain_world.preflight(args)
                args.source_root = root / 'missing'
                with self.assertRaisesRegex(ValueError, 'must exist'):
                    terrain_world.preflight(args)

    def test_first_world_publication_checks_ocean_before_writing_coastal_land(self):
        def window(binary, x, y):
            return dict(north_node=10-2*y, west_node=2*x, rows=3, columns=3, nodes_per_degree=1,
                        dem_codes_per_metre=5, dem_offset_m=-500, dem_missing=65535)
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); binary = root / 'binary'; binary.write_bytes(b'fixture')
            plan = root / 'plan.json'
            plan.write_text(json.dumps(dict(channel='dem', squares=[[2, 2]], sources=[
                dict(path='fixture', role='fallback', group='one', epoch='2020')])))
            coverage = dict(sha256='reviewed', squares=[dict(x=1, y=1, status='ocean')])
            with (
                patch.object(terrain_produce, 'provenance', return_value={}),
                patch.object(terrain_produce, 'grouped_sources', side_effect=lambda s: s),
                patch.object(terrain_produce, 'raster_window', side_effect=window),
                patch.object(terrain_produce, 'assemble_with_statistics', return_value=(
                    np.full((3, 3), 20.), np.zeros((3, 3)), dict(maximum_artificial_step_bound_m=.2))),
            ):
                with self.assertRaisesRegex(ValueError, 'shared square edge'):
                    terrain_produce.produce(plan, root / 'output', binary, 0, coverage)
                terrain_produce.produce(plan, root / 'standalone', binary, 0)
                with self.assertRaisesRegex(ValueError, 'differs from resumed plan'):
                    terrain_produce.produce(plan, root / 'standalone', binary, 0, coverage)
            self.assertTrue((root / 'output/z9/1/1/dem.u16le').exists())
            self.assertFalse((root / 'output/z9/2/2/dem.u16le').exists())

    def test_derived_rasters_obey_the_combined_retained_byte_cap(self):
        spec = importlib.util.spec_from_file_location('bavaria', Path(__file__).with_name('fetch-bavaria.py'))
        bavaria = importlib.util.module_from_spec(spec); spec.loader.exec_module(bavaria)
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); provider = root / 'provider'; provider.mkdir()
            source = provider / 'sample.zip'
            with zipfile.ZipFile(source, 'w') as archive:
                archive.writestr('sample.txt', '500000 5500000 100\n500005 5500000 101\n'
                                  '500000 5499995 102\n500005 5499995 103\n')
            terrain_io.publish_json(str(source) + '.provenance.json', dict(url='https://example.org/sample',
                fetched_utc='2026-09-24', sha256=terrain_io.digest(source), bytes=source.stat().st_size,
                licence='fixture', licence_url='https://example.org/licence', terms_checked_utc='2026-09-24'))
            before = sum(p.stat().st_size for p in root.rglob('*') if p.is_file())
            with patch.object(terrain_io, 'MAX_DOWNLOAD_BYTES', before + 1):
                with self.assertRaisesRegex(ValueError, 'shared source budget'):
                    bavaria.decode_archive(source)
                with self.assertRaisesRegex(ValueError, 'source manifest exceeds'):
                    terrain_io.publish_source_json(root, provider / 'country-sources.json', ['fixture'])
            self.assertFalse(source.with_suffix('.tif').exists())
            bavaria.decode_archive(source)
            self.assertTrue(source.with_suffix('.tif').exists())
            terrain_io.provenance(source.with_suffix('.tif'))


if __name__ == '__main__':
    unittest.main()
