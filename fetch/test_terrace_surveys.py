"""Seating survey normalization: Madrid's kinds by activity priority and open terraces, Melbourne's pubs."""
from pathlib import Path
import tempfile
import unittest

from terrace_surveys import madrid, melbourne


def write(path, header, rows):
    path.write_text('﻿' + ';'.join(header) + '\n'
                    + ''.join(';'.join(f'"{cell}"' for cell in row) + '\n' for row in rows),
                    encoding='utf-8')


class TerraceSurveyTests(unittest.TestCase):
    def test_madrid_takes_open_premises_by_kind_priority_and_the_chairs_of_open_terraces(self):
        with tempfile.TemporaryDirectory() as temporary:
            edition = Path(temporary)
            write(edition / 'locales.csv', ['id_local', 'desc_situacion_local'],
                  [['1', 'Abierto'], ['2', 'Abierto'], ['3', 'Cerrado'], ['4', 'Abierto'],
                   ['5', 'Abierto'], ['6', 'Abierto']])
            write(edition / 'actividades.csv', ['id_local', 'id_epigrafe'],
                  [['1', '561001'], ['1', '561005'], ['2', '561006'], ['3', '561001'],
                   ['4', '563002'], ['4', '561005'], ['5', '932006'], ['6', '471100']])
            write(edition / 'terrazas.csv', ['id_local', 'desc_situacion_terraza', 'sillas_es'],
                  [['1', 'Abierta', '20'], ['1', 'Abierta', '8'], ['2', 'Suspension temporal', '12'],
                   ['3', 'Abierta', '30']])
            # A bar's code ranks before a restaurant's; a late bar and a shop are left out; a
            # suspended terrace has no chairs; a closed premise is not surveyed.
            self.assertEqual(madrid(edition), [('bar', 28.0), ('cafe', 0.0), ('nightclub', 0.0)])

    def test_melbourne_sums_each_pubs_outdoor_seats_in_its_census_year(self):
        with tempfile.TemporaryDirectory() as temporary:
            seats = Path(temporary) / 'seats.csv'
            header = ['census_year', 'property_id', 'trading_name', 'business_address',
                      'industry_anzsic4_description', 'seating_type', 'number_of_seats']
            pub = ['Pubs, Taverns and Bars']
            write(seats, header, [
                ['2024', '1', 'Arms', '1 Lane'] + pub + ['Seats - Outdoor', '30'],
                ['2024', '1', 'Arms', '1 Lane'] + pub + ['Seats - Outdoor', '10'],
                ['2024', '1', 'Arms', '1 Lane'] + pub + ['Seats - Indoor', '90'],
                ['2024', '2', 'Tavern', '2 Lane'] + pub + ['Seats - Indoor', '60'],
                ['2023', '3', 'Old', '3 Lane'] + pub + ['Seats - Outdoor', '50'],
                ['2024', '4', 'Cafe', '4 Lane', 'Cafes and Restaurants', 'Seats - Outdoor', '20'],
            ])
            self.assertEqual(melbourne(seats, '2024'), [('pub', 40.0), ('pub', 0.0)])


if __name__ == '__main__':
    unittest.main()
