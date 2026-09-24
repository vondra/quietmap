"""The height ladder's rungs, the demand storeys derived from it and national wall defaults."""

import math
import unittest

from test_structures_fixtures import CONTRACT
import structure_heights as HEIGHTS


def ladder(regional=None, osm_height=None, floors=0, overture=None, ghsl=math.nan, area=100.0):
    return HEIGHTS.screening_height_and_source(regional, osm_height, floors, overture, ghsl, area)


class HeightLadderTests(unittest.TestCase):
    def test_first_available_rung_wins(self):
        self.assertEqual(ladder(regional=300.0, osm_height=9.0), (250.0, CONTRACT.HEIGHT_SOURCE_REGIONAL_MEASURED))
        self.assertEqual(ladder(osm_height=9.0, floors=5), (9.0, CONTRACT.HEIGHT_SOURCE_OSM_HEIGHT))
        self.assertEqual(ladder(floors=1, overture=20.0), (6.0, CONTRACT.HEIGHT_SOURCE_FLOORS))
        self.assertEqual(ladder(overture=20.0, ghsl=9.0), (20.0, CONTRACT.HEIGHT_SOURCE_OVERTURE_HEIGHT))
        self.assertEqual(ladder(ghsl=9.0), (9.0, CONTRACT.HEIGHT_SOURCE_GHSL))
        self.assertEqual(ladder(), (7.4, CONTRACT.HEIGHT_SOURCE_AREA_TYPOLOGY))

    def test_low_overture_heights_and_the_ghsl_floor_are_no_information(self):
        """1,002,248 Overture footprints screened at 0 m; 1.3 billion GHSL rows at 3 m."""
        self.assertEqual(ladder(overture=0.0, area=40.0), (5.4, CONTRACT.HEIGHT_SOURCE_AREA_TYPOLOGY))
        self.assertEqual(ladder(overture=2.4, ghsl=2.5, area=700.0),
                         (10.6, CONTRACT.HEIGHT_SOURCE_AREA_TYPOLOGY))
        self.assertEqual(ladder(overture=2.5), (2.5, CONTRACT.HEIGHT_SOURCE_OVERTURE_HEIGHT))
        self.assertEqual(ladder(ghsl=3.49), (7.4, CONTRACT.HEIGHT_SOURCE_AREA_TYPOLOGY))
        self.assertEqual(ladder(ghsl=3.5), (3.5, CONTRACT.HEIGHT_SOURCE_GHSL))
        self.assertEqual(ladder(ghsl=250.0), (100.0, CONTRACT.HEIGHT_SOURCE_GHSL))
        self.assertFalse(HEIGHTS.needs_ghsl(None, 0, 2.5))
        self.assertTrue(HEIGHTS.needs_ghsl(0.0, 0, 2.4))

    def test_a_cell_average_never_raises_a_shed_above_four_metres(self):
        self.assertEqual(ladder(ghsl=15.0, area=29.9), (4.0, CONTRACT.HEIGHT_SOURCE_GHSL))
        self.assertEqual(ladder(ghsl=15.0, area=30.0), (15.0, CONTRACT.HEIGHT_SOURCE_GHSL))
        self.assertEqual(ladder(area=29.9), (2.9, CONTRACT.HEIGHT_SOURCE_AREA_TYPOLOGY))

    def test_demand_storeys_invert_the_floors_rung(self):
        for floors in (1, 2, 7):
            height, _ = ladder(floors=floors)
            self.assertEqual(HEIGHTS.demand_storeys_and_source(0, height),
                             (floors, CONTRACT.STOREYS_SOURCE_LADDER_HEIGHT))
        self.assertEqual(HEIGHTS.demand_storeys_and_source(3, 30.0), (3, CONTRACT.STOREYS_SOURCE_FLOORS))
        self.assertEqual(HEIGHTS.demand_storeys_and_source(0, 2.9), (1, CONTRACT.STOREYS_SOURCE_LADDER_HEIGHT))
        self.assertEqual(HEIGHTS.demand_storeys_and_source(0, None), (1, CONTRACT.STOREYS_SOURCE_SINGLE_LEVEL))

    def test_unmapped_walls_stand_at_their_national_mean(self):
        iso = lambda code: ord(code[0]) | ord(code[1]) << 8
        self.assertEqual(HEIGHTS.wall_height_and_source(3.0, False, iso("DE")), (3.88, CONTRACT.HEIGHT_SOURCE_WALL_DEFAULT))
        self.assertEqual(HEIGHTS.wall_height_and_source(3.0, False, iso("US")), (4.45, CONTRACT.HEIGHT_SOURCE_WALL_DEFAULT))
        self.assertEqual(HEIGHTS.wall_height_and_source(0.0, False, 0), (3.0, CONTRACT.HEIGHT_SOURCE_WALL_DEFAULT))
        self.assertEqual(HEIGHTS.wall_height_and_source(2.2, True, iso("DE")), (2.2, CONTRACT.HEIGHT_SOURCE_OSM_HEIGHT))


if __name__ == "__main__":
    unittest.main(verbosity=2)
