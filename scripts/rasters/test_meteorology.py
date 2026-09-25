"""Protect the meteorology producer's direction, absorption, crash-restart and square-publication invariants."""
import importlib.util
import json
from pathlib import Path
import struct
import sys
import tempfile
import unittest

# Heavy producer dependencies are installed by the raster environment.
AVAILABLE = all(importlib.util.find_spec(name) for name in ('numba', 'numcodecs'))
if AVAILABLE:
    import numpy as np
    sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'engine/noise-compute'))
    from meteorology import (accumulate, alpha, cell_coordinates, empty_state, favourable,
                             periods_from_zone_hours, relative_humidity)
    from meteorology_io import (Checkpoint, era5_window, final_nodes, meteorology_magic,
                                square_file_bytes, square_node_indices, timezone_rules,
                                verify_squares, write_squares)


@unittest.skipUnless(AVAILABLE, 'meteorology requires numba and numcodecs')
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

    def test_checkpoint_replay_and_square_encoding(self):
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
                probabilities, means, variances = final_nodes(loaded)
                self.assertEqual(probabilities[0, 0].tolist(), [70] * 16)
                self.assertEqual(means[0, 0].tolist(), [4.] * 8)
                self.assertEqual(variances[0, 2].tolist(), [2.] * 8)
                loaded['counts'][0, 1] = 0
                with self.assertRaisesRegex(ValueError, 'unobserved'):
                    final_nodes(loaded)
            cells = 721 * 1440
            prob = np.zeros((cells, 3, 16), dtype=np.uint8)
            mean = np.zeros((cells, 3, 8), dtype=np.float32)
            variance = np.zeros((cells, 3, 8), dtype=np.float32)
            subset = [(276, 173), (0, 256), (511, 256), (256, 0), (256, 511)]
            for x, y in subset:
                _, indices = square_node_indices(x, y)
                prob[indices] = 70
                mean[indices] = 4
                variance[indices] = 2
            published = write_squares(root / 'rasters', prob, mean, variance, subset)
            self.assertEqual(published[0], len(subset))
            for x, y in subset:
                window, _ = square_node_indices(x, y)
                west, north, rows, columns = window
                raw = (root / 'rasters' / 'z9' / str(x) / str(y) / 'meteorology.bin').read_bytes()
                self.assertEqual(raw[:8], meteorology_magic())
                self.assertEqual(struct.unpack('<2h2H', raw[8:16]), (west, north, columns, rows))
                self.assertEqual(len(raw), 16 + rows * columns * 240)
                self.assertEqual(raw[16:64], bytes([70]) * 48)
                self.assertEqual(raw[64:68], struct.pack('<f', 4.0))
                self.assertEqual(raw[160:164], struct.pack('<f', 2.0))
            self.assertEqual(verify_squares(root / 'rasters', subset), published)
            with self.assertRaisesRegex(ValueError, 'do not match'):
                square_file_bytes((0, 0, 4, 4), prob[:15], mean[:15], variance[:15])
            (root / 'rasters' / 'z9' / '276' / '173' / 'meteorology.bin').write_bytes(b'short')
            with self.assertRaisesRegex(ValueError, 'failed verification'):
                verify_squares(root / 'rasters', subset)

    def test_era5_windows_match_reader_goldens(self):
        for square, expected in [((276, 173), (56, 202, 4, 5)),
                                 ((256, 256), (0, 0, 4, 4)),
                                 ((278, 71), (61, 313, 2, 5)),
                                 ((256, 0), (0, 360, 22, 4)),
                                 ((256, 511), (0, -339, 22, 4)),
                                 ((0, 256), (-720, 0, 4, 4)),
                                 ((511, 256), (717, 0, 4, 4))]:
            self.assertEqual(era5_window(*square), expected, f'square {square}')
        with self.assertRaisesRegex(ValueError, 'out of range'):
            era5_window(512, 0)

    def test_cell_grid_and_period_boundaries(self):
        latitudes, longitudes = cell_coordinates(721 * 1440)
        self.assertEqual((latitudes[0], latitudes[-1]), (90., -90.))
        self.assertEqual((longitudes[0], longitudes[1439]), (0., 359.75))
        self.assertEqual(periods_from_zone_hours(np.arange(24)).tolist(),
                         [2] * 7 + [0] * 12 + [1] * 4 + [2])

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
