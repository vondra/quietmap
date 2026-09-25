"""Protect natural slopes while rejecting artificial source and square-edge discontinuities."""
import unittest
import tempfile
from pathlib import Path
from terrain_io import publish_json, digest
import numpy as np
from terrain_seams import feather, artificial_steps, require_seam_gate, verify_shared_nodes


class SeamTest(unittest.TestCase):
    def test_halo_is_independent_of_square_extent_and_preserves_natural_slope(self):
        h = 16
        base = np.broadcast_to(np.arange(200.) * 20, (100, 200)).copy()
        national = base + 2
        national[:, :50] = np.nan
        full, weight, difference = feather(base, national, h)
        part, _, _ = feather(base[10:90, 20:150], national[10:90, 20:150], h)
        np.testing.assert_array_equal(part[20:-20, 20:-20], full[30:70, 40:130])
        stats = artificial_steps(weight, difference, h)
        require_seam_gate(stats)
        self.assertGreater(float(np.max(np.diff(full[50]))), 20)
        self.assertLessEqual(stats['maximum_artificial_step_bound_m'], .5)

    def test_disagreement_that_needs_a_wider_halo_fails_the_writer_gate(self):
        base = np.zeros((100, 100))
        national = np.full_like(base, 100)
        national[:, :40] = np.nan
        _, weight, difference = feather(base, national, 16)
        with self.assertRaisesRegex(ValueError, 'artificial source seam'):
            require_seam_gate(artificial_steps(weight, difference, 16))

    def test_diagonal_nodes_and_verified_empty_ocean_are_checked(self):
        def window(binary, x, y):
            return dict(north_node=10-2*y, west_node=2*x, rows=3, columns=3,
                        nodes_per_degree=1, dem_codes_per_metre=5, dem_offset_m=-500)
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'z9/2/2/dem.u16le'
            neighbour = Path(temp) / 'z9/1/1/dem.u16le'
            neighbour.parent.mkdir(parents=True)
            neighbour.write_bytes(np.full((3, 3), 2600, dtype='<u2').tobytes())
            receipt = Path(str(neighbour) + '.provenance.json')
            publish_json(receipt, dict(plan_sha256='same', sha256=digest(neighbour)))
            codes = np.full((3, 3), 2500, dtype='<u2')
            with self.assertRaisesRegex(ValueError, 'shared square edge'):
                verify_shared_nodes(path, codes, window(None, 2, 2), None, 'same', window)
            neighbour.write_bytes(b''); receipt.unlink()
            publish_json(receipt, dict(plan_sha256='same', sha256=digest(neighbour), coverage_verified_ocean=True))
            result = verify_shared_nodes(path, codes, window(None, 2, 2), None, 'same', window)
            self.assertEqual(result['shared_nodes'], 1)
            self.assertEqual(result['maximum_shared_node_difference_m'], 0)
            with self.assertRaisesRegex(ValueError, 'shared square edge'):
                verify_shared_nodes(path, codes + 3, window(None, 2, 2), None, 'same', window)

    def test_missing_fallback_cannot_be_hidden_by_national_precedence(self):
        with self.assertRaisesRegex(ValueError, 'finite fallback'):
            feather(np.full((4, 4), np.nan), np.ones((4, 4)), 2)


if __name__ == '__main__':
    unittest.main()
