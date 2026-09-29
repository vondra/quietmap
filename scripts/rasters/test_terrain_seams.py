"""Protect natural slopes while rejecting artificial source and square-edge discontinuities."""
import unittest
import tempfile
from pathlib import Path
from terrain_io import publish_json, digest
import numpy as np
from terrain_seams import (feather, artificial_steps, exclusion_edges, require_seam_gate,
                           verify_shared_nodes)


class SeamTest(unittest.TestCase):
    def test_halo_is_independent_of_square_extent_and_preserves_natural_slope(self):
        h = 16
        base = np.broadcast_to(np.arange(200.) * 20, (100, 200)).copy()
        national = base + 2
        national[:, :50] = np.nan
        full, weight, difference = feather(base, national, h)
        part, _, _ = feather(base[10:90, 20:150], national[10:90, 20:150], h)
        np.testing.assert_array_equal(part[20:-20, 20:-20], full[30:70, 40:130])
        stats, _, _ = artificial_steps(weight, difference, h, national, base)
        require_seam_gate(stats)
        self.assertGreater(float(np.max(np.diff(full[50]))), 20)
        self.assertLessEqual(stats['maximum_artificial_step_bound_m'], .5)

    def test_disagreement_that_needs_a_wider_halo_fails_the_writer_gate(self):
        base = np.zeros((100, 100))
        national = np.full_like(base, 100)
        national[:, :40] = np.nan
        _, weight, difference = feather(base, national, 16)
        with self.assertRaisesRegex(ValueError, 'artificial source seam'):
            require_seam_gate(artificial_steps(weight, difference, 16, national, base)[0])

    def test_cliff_ramp_hides_in_the_slope_but_capped_ramps_still_trip(self):
        base = np.broadcast_to(np.arange(100.) * 50, (60, 100)).copy()
        national = base + 10
        national[:, :40] = np.nan
        _, weight, difference = feather(base, national, 16)
        stats, _, _ = artificial_steps(weight, difference, 18, national, base)
        self.assertGreater(stats['maximum_selection_step_m'], .3)
        self.assertGreater(stats['selection_exempt_edges'], 0)
        require_seam_gate(stats)
        national = base + 30
        national[:, :40] = np.nan
        _, weight, difference = feather(base, national, 16)
        with self.assertRaisesRegex(ValueError, 'artificial source seam'):
            require_seam_gate(artificial_steps(weight, difference, 18, national, base)[0])

    def test_along_strike_cliff_ramp_exempts_on_cross_axis_relief(self):
        # Tenerife's west cliff runs along the edge: axial slope is flat but the
        # face drops 50 m/node across it, so the 10 m ramp hides like an
        # across-strike one; a flat-terrain ramp of the same size still trips.
        base = np.broadcast_to((np.arange(100.) * 50)[:, None], (100, 100)).copy()
        national = base + 10
        national[:, :40] = np.nan
        _, weight, difference = feather(base, national, 16)
        stats, _, _ = artificial_steps(weight, difference, 18, national, base)
        self.assertGreater(stats['maximum_selection_step_m'], .3)
        self.assertGreater(stats['selection_exempt_edges'], 0)
        require_seam_gate(stats)
        flat = np.zeros((100, 100))
        national = np.full_like(flat, 10)
        national[:, :40] = np.nan
        _, weight, difference = feather(flat, national, 16)
        with self.assertRaisesRegex(ValueError, 'artificial source seam'):
            require_seam_gate(artificial_steps(weight, difference, 18, national, flat)[0])

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

    def test_cross_plan_neighbours_compare_values_and_stay_recorded(self):
        def window(binary, x, y):
            return dict(north_node=10-2*y, west_node=2*x, rows=3, columns=3,
                        nodes_per_degree=1, dem_codes_per_metre=5, dem_offset_m=-500)
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'z9/2/2/dem.u16le'
            neighbour = Path(temp) / 'z9/1/1/dem.u16le'
            neighbour.parent.mkdir(parents=True)
            neighbour.write_bytes(np.full((3, 3), 2501, dtype='<u2').tobytes())
            receipt = Path(str(neighbour) + '.provenance.json')
            publish_json(receipt, dict(plan_sha256='other-plan', sha256=digest(neighbour)))
            codes = np.full((3, 3), 2500, dtype='<u2')
            result = verify_shared_nodes(path, codes, window(None, 2, 2), None, 'same', window)
            self.assertEqual(result['cross_plan_neighbours'], [[1, 1]])
            with self.assertRaisesRegex(ValueError, 'shared square edge'):
                verify_shared_nodes(path, codes + 4, window(None, 2, 2), None, 'same', window)

    def test_empty_ocean_neighbours_bind_to_the_coverage_manifest_not_a_plan(self):
        def window(binary, x, y):
            return dict(north_node=10-2*y, west_node=2*x, rows=3, columns=3,
                        nodes_per_degree=1, dem_codes_per_metre=5, dem_offset_m=-500)
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'z9/2/2/dem.u16le'
            neighbour = Path(temp) / 'z9/1/1/dem.u16le'
            neighbour.parent.mkdir(parents=True)
            neighbour.write_bytes(b'')
            receipt = Path(str(neighbour) + '.provenance.json')
            publish_json(receipt, dict(channel='dem', sha256=digest(neighbour),
                                       coverage_verified_ocean=True, coverage_manifest_sha256='reviewed'))
            codes = np.full((3, 3), 2500, dtype='<u2')
            result = verify_shared_nodes(path, codes, window(None, 2, 2), None, 'any-plan', window, 'reviewed')
            self.assertEqual(result['shared_nodes'], 1)
            with self.assertRaisesRegex(ValueError, 'ocean coverage differs'):
                verify_shared_nodes(path, codes, window(None, 2, 2), None, 'any-plan', window, 'other')

    def test_stacked_bound_sums_per_edge_not_per_group_maxima(self):
        halo = 2
        flat = np.zeros((8, 8))
        first_weight = np.zeros((8, 8))
        first_weight[:, 4:] = 1.
        _, _, first = artificial_steps(first_weight, np.full((8, 8), .4), halo, flat, flat)
        second_weight = np.zeros((8, 8))
        second_weight[:, 6:] = 1.
        _, _, second = artificial_steps(second_weight, np.full((8, 8), .4), halo, flat, flat)
        # Each group peaks at 0.4 on its own edge; the edges never coincide.
        self.assertAlmostEqual(float(np.maximum(first, second).max()), .4, places=6)
        self.assertAlmostEqual(float((first + second).max()), .4, places=6)
        # Coinciding worst cases still sum at their shared edge.
        self.assertAlmostEqual(float((first + first).max()), .8, places=6)

    def test_waiver_disc_excludes_only_edges_touching_it(self):
        # 8x8 nodes over 0.008 degrees: 111 m node spacing at the equator.
        masks = exclusion_edges(8, 8, (0., 0., .008, .008), [(.0045, .0035, 10.)])
        # Only node (4, 4) sits inside the 10 m disc; each orientation keeps
        # the two edges touching it.
        self.assertEqual([int(mask.sum()) for mask in masks], [2, 2])
        self.assertTrue(masks[0][3, 4] and masks[0][4, 4])
        self.assertTrue(masks[1][4, 3] and masks[1][4, 4])
        halo = 2
        flat = np.zeros((8, 8))
        tall = np.zeros((8, 8))
        tall[:, 4:] = 1.
        exclude = [np.zeros((7, 8), bool), np.zeros((8, 7), bool)]
        exclude[1][:, 3] = True
        steps, stack0, stack1 = artificial_steps(tall, np.full((8, 8), .4), halo, flat, flat,
                                                 exclude)
        self.assertEqual(steps['evaluated_transition_edges'], 0)
        self.assertEqual(steps['maximum_selection_step_m'], 0)
        self.assertEqual(stack0.max(), 0)
        self.assertEqual(stack1.max(), 0)


if __name__ == '__main__':
    unittest.main()
