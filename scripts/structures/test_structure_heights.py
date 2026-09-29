"""Unit tests for the one screening-height ladder (mean roof heights)."""

import unittest

from structure_contract import (
    HEIGHT_SOURCE_OSM_HEIGHT, HEIGHT_SOURCE_FLOORS, HEIGHT_SOURCE_AREA_TYPOLOGY,
    HEIGHT_SOURCE_REGIONAL_MEASURED, HEIGHT_SOURCE_OVERTURE_HEIGHT,
    HEIGHT_SOURCE_WALL_DEFAULT, STOREYS_SOURCE_FLOORS, STOREYS_SOURCE_LADDER_HEIGHT,
    STOREYS_SOURCE_SINGLE_LEVEL,
)
from structure_heights import (
    demand_storeys_and_source, floors_height_m, screening_height_and_source,
    wall_height_and_source,
)


class HeightLadderTests(unittest.TestCase):
    def test_first_available_rung_wins(self):
        ladder = screening_height_and_source
        self.assertEqual(ladder(12.5, 9.0, 3, 10.0, 100.0),
                         (12.5, HEIGHT_SOURCE_REGIONAL_MEASURED))
        self.assertEqual(ladder(None, 9.0, 3, 10.0, 100.0), (9.0, HEIGHT_SOURCE_OSM_HEIGHT))
        self.assertEqual(ladder(None, None, 3, 10.0, 100.0), (9.0, HEIGHT_SOURCE_FLOORS))
        self.assertEqual(ladder(None, None, 0, 10.0, 100.0),
                         (10.0, HEIGHT_SOURCE_OVERTURE_HEIGHT))
        self.assertEqual(ladder(None, None, 0, None, 100.0),
                         (7.4, HEIGHT_SOURCE_AREA_TYPOLOGY))

    def test_low_overture_heights_fall_through_to_the_typology(self):
        self.assertEqual(screening_height_and_source(None, None, 0, 2.4, 100.0),
                         (7.4, HEIGHT_SOURCE_AREA_TYPOLOGY))
        self.assertEqual(screening_height_and_source(None, None, 0, 0.0, 20.0),
                         (2.9, HEIGHT_SOURCE_AREA_TYPOLOGY))

    def test_floors_count_the_attic_once_and_the_roof_above_three(self):
        self.assertEqual([(floors, floors_height_m(floors)) for floors in (1, 2, 3, 4, 5)],
                         [(1, 6.0), (2, 6.0), (3, 9.0), (4, 14.0), (5, 17.0)])

    def test_typology_bins_are_the_no_information_medians(self):
        ladder = screening_height_and_source
        cases = [(29.9, 2.9), (30.0, 3.5), (59.9, 3.5), (60.0, 7.4),
                 (149.9, 7.4), (150.0, 8.0), (499.9, 8.0), (500.0, 9.0), (1e9, 9.0)]
        for footprint, expected in cases:
            with self.subTest(footprint=footprint):
                self.assertEqual(ladder(None, None, 0, None, footprint),
                                 (expected, HEIGHT_SOURCE_AREA_TYPOLOGY))

    def test_regional_heights_clamp_to_the_survey_range(self):
        self.assertEqual(screening_height_and_source(1.0, None, 0, None, 100.0),
                         (2.5, HEIGHT_SOURCE_REGIONAL_MEASURED))
        self.assertEqual(screening_height_and_source(999.0, None, 0, None, 100.0),
                         (250.0, HEIGHT_SOURCE_REGIONAL_MEASURED))

    def test_demand_storeys_prefer_floors_then_estimate_from_height(self):
        storeys = demand_storeys_and_source
        self.assertEqual(storeys(5, 17.0), (5, STOREYS_SOURCE_FLOORS))
        self.assertEqual(storeys(0, None), (1, STOREYS_SOURCE_SINGLE_LEVEL))
        for height, expected in [(2.9, 1), (4.0, 1), (6.0, 2), (7.4, 2),
                                 (9.0, 3), (11.0, 3), (14.0, 4), (26.0, 8)]:
            with self.subTest(height=height):
                self.assertEqual(storeys(0, height), (expected, STOREYS_SOURCE_LADDER_HEIGHT))

    def test_unmapped_walls_stand_at_their_national_mean(self):
        germany = ord("D") | ord("E") << 8
        self.assertEqual(wall_height_and_source(0.0, False, germany),
                         (3.88, HEIGHT_SOURCE_WALL_DEFAULT))
        self.assertEqual(wall_height_and_source(2.5, True, germany),
                         (2.5, HEIGHT_SOURCE_OSM_HEIGHT))


if __name__ == "__main__":
    unittest.main(verbosity=2)
