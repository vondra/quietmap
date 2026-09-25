"""Protect the meteorology producer's direction, absorption and crash-restart invariants."""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest

# Heavy producer dependencies are installed by the raster environment.
AVAILABLE = all(importlib.util.find_spec(name) for name in ('numba', 'numcodecs', 'pyarrow'))
if AVAILABLE:
    import numpy as np
    sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'engine/noise-compute'))
    from meteorology import alpha, accumulate, empty_state, favourable, relative_humidity
    from meteorology_io import Checkpoint, timezone_rules, write_arrow
    import pyarrow.ipc as ipc


@unittest.skipUnless(AVAILABLE, 'meteorology requires numba, numcodecs and pyarrow')
class MeteorologyTests(unittest.TestCase):
    def test_direction_and_population_moments(self):
        state = empty_state(1)
        # Northward neutral wind favours northward sound, not a source to the north.
        values = np.array([[0.], [5.], [288.15], [70.], [1.], [101.325]])
        for temperature in [283.15, 293.15]:
            values[2] = temperature
            accumulate(values, np.array([0], dtype=np.uint8), np.array([True]), **state)
        self.assertEqual(state['favourable_counts'][0, 0, 0], 2)
        self.assertEqual(state['favourable_counts'][0, 0, 8], 0)
        expected = np.array([alpha(1000., t, 70., 101.325) for t in [283.15, 293.15]])
        self.assertAlmostEqual(state['means'][0, 0, 4], expected.mean())
        self.assertAlmostEqual(state['m2'][0, 0, 4] / 2, expected.var())
        self.assertEqual(state['histograms'].sum(), state['counts'].sum())
        self.assertAlmostEqual(relative_humidity(280., 280.), 100.)
        # Independent ISO 9613-1 reference calculation at 20 C, 70 %, 1 kHz.
        self.assertAlmostEqual(alpha(1000., 293.15, 70., 101.325), 4.977810847, places=6)
        self.assertTrue(favourable(0, 4, 0.))  # calm clear night, inversion
        self.assertFalse(favourable(0, 0, 0.))

    def test_checkpoint_replay_and_arrow_encoding(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            state = empty_state(1)
            state['counts'][:] = 10
            state['favourable_counts'][:] = 7
            state['means'][:] = 4
            state['m2'][:] = 20
            checkpoint = Checkpoint(root, {'source': 'test'})
            with (root / 'chunks.jsonl').open('w+b') as manifest:
                manifest.write(b'first\n')
                checkpoint.save(state, 10, manifest)
                manifest.write(b'uncommitted\n')
                manifest.flush()
                loaded, step, offset = checkpoint.load()
                self.assertEqual((step, offset), (10, 6))
                # An interrupted inactive-slot write never replaces the committed pointer.
                (root / 'state-1.npz').write_bytes(b'crash')
                loaded, step, offset = checkpoint.load()
                for key in state:
                    np.testing.assert_array_equal(loaded[key], state[key])
                with self.assertRaisesRegex(ValueError, 'identity changed'):
                    Checkpoint(root, {'source': 'other'}).load()
                committed = (root / 'chunks.jsonl').read_bytes()
                (root / 'chunks.jsonl').write_bytes(b'broken')
                with self.assertRaisesRegex(ValueError, 'checksum mismatch'):
                    checkpoint.load()
                (root / 'chunks.jsonl').write_bytes(b'')
                with self.assertRaisesRegex(ValueError, 'truncated'):
                    checkpoint.load()
                (root / 'chunks.jsonl').write_bytes(committed)
                output = root / 'pilot.arrow'
                write_arrow(output, loaded, np.array([160 * 1440 + 57]), {'complete': 'false', 'source_identity': 'test'})
                table = ipc.open_file(output).read_all()
                self.assertEqual(table['p_day'][0].as_py(), [70] * 16)
                self.assertEqual(table['alpha_variance_night'][0].as_py(), [2.] * 8)
                loaded['counts'][0, 1] = 0
                with self.assertRaisesRegex(ValueError, 'unobserved'):
                    write_arrow(output, loaded, np.array([0]), {})

    def test_timezone_snapshot_is_reused_and_bound_to_identity(self):
        from datetime import datetime, timezone
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            zones, digest = timezone_rules(['Europe/Prague'], root)
            with patch('meteorology_io.TZPATH', ()):
                restored, restored_digest = timezone_rules(['Europe/Prague'], root)
            self.assertEqual(restored_digest, digest)
            for timestamp, hour in [(datetime(1991, 1, 1, 18, tzinfo=timezone.utc), 19),
                                    (datetime(2020, 7, 1, 17, tzinfo=timezone.utc), 19)]:
                self.assertEqual(timestamp.astimezone(restored[0]).hour, hour)
                self.assertEqual(timestamp.astimezone(zones[0]), timestamp.astimezone(restored[0]))


if __name__ == '__main__':
    unittest.main()
