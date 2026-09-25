"""The survey raster answers footprints it covers; anywhere else the row keeps no regional mean."""

import unittest
from types import SimpleNamespace

import shapely

from test_structures_fixtures import SOURCES


class RegionalSamplingTests(unittest.TestCase):
    def test_survey_answers_covered_footprints_and_abstains_elsewhere(self):
        rows = [dict(clon=float(i), clat=0.0, geom=None if i == 2 else shapely.Point(i, 0))
                for i in range(4)]
        regional = SimpleNamespace(
            tr=SimpleNamespace(transform=lambda xs, ys: (xs, ys)),
            covers=lambda x, _y: x < 3,
            zonal_measured_mean=lambda geometry: {1: 20.5}.get(int(geometry.x)))
        stats = dict(regional=0, abstain=0)
        SOURCES.sample_regional_heights(rows, regional, stats)
        self.assertEqual([row["regional_m"] for row in rows],
                         [None, 20.5, None, None])
        self.assertEqual(stats, dict(regional=1, abstain=1))

    def test_no_survey_leaves_every_row_without_a_regional_mean(self):
        rows = [dict(clon=0.0, clat=0.0, geom=shapely.Point(0, 0))]
        stats = dict(regional=0, abstain=0)
        SOURCES.sample_regional_heights(rows, None, stats)
        self.assertEqual(rows[0]["regional_m"], None)
        self.assertEqual(stats, dict(regional=0, abstain=0))


if __name__ == '__main__':
    unittest.main(verbosity=2)
