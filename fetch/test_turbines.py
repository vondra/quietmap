"""Historical cutoff filtering, post-cutoff dismantlement restoration and undated preservation for turbines."""
import datetime
import io
import json
import os
import subprocess
import tempfile
import unittest
from unittest import mock

import turbines


CUTOFF = datetime.date(2025, 1, 1)


def osm_feature(osm_id, lat, lon):
    return json.dumps({
        'type': 'Feature',
        'properties': {'@type': 'node', '@id': osm_id, 'generator:source': 'wind'},
        'geometry': {'type': 'Point', 'coordinates': [lon, lat]},
    }) + '\n'


class TurbinesAsOfTest(unittest.TestCase):
    def test_uswtdb_excludes_post_cutoff_and_blanks_post_cutoff_retrofits(self):
        with tempfile.TemporaryDirectory() as root:
            os.makedirs(f'{root}/uswtdb')
            with open(f'{root}/uswtdb/uswtdb_v7_2.csv', 'w', encoding='utf-8') as f:
                f.write('ylat,xlong,t_hh,t_cap,p_year,t_retro_yr\n')
                f.write('40.0,-100.0,80,2000,2024,-9999\n')
                f.write('41.0,-100.0,90,3000,2025,-9999\n')
                f.write('42.0,-100.0,70,1500,-9999,-9999\n')
                f.write('43.0,-100.0,110,4500,2012,2025\n')
                f.write('44.0,-100.0,105,3600,2012,2023\n')
            rows = list(turbines.uswtdb(root, as_of=CUTOFF))
            self.assertEqual(
                [(r[1], r[3], r[4]) for r in rows],
                [(40.0, 80.0, 2000.0), (42.0, 70.0, 1500.0), (43.0, 0.0, 0.0), (44.0, 105.0, 3600.0)],
            )
            self.assertEqual(len(list(turbines.uswtdb(root))), 5)

    def test_turbines_shell_refuses_as_of_without_pinned_registers(self):
        script = os.path.join(os.path.dirname(os.path.abspath(turbines.__file__)), 'turbines.sh')
        env = {k: v for k, v in os.environ.items() if k != 'TURBINE_REGISTERS'}
        result = subprocess.run(
            ['bash', script, '/nonexistent/planet.osm.pbf', '/nonexistent/out', '--as-of', '2025-01-01'],
            env=env, capture_output=True, text=True, check=False,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('TURBINE_REGISTERS', result.stderr)

    def test_mastr_filters_future_and_restores_post_cutoff_shutdowns_before_pairing(self):
        with tempfile.TemporaryDirectory() as root:
            os.makedirs(f'{root}/mastr')
            with open(f'{root}/mastr/EinheitenWind_1.xml', 'w', encoding='utf-8') as f:
                f.write('''<EinheitenWind>
  <EinheitWind>
    <EinheitBetriebsstatus>35</EinheitBetriebsstatus>
    <Breitengrad>52.0</Breitengrad><Laengengrad>10.0</Laengengrad>
    <Nabenhoehe>120</Nabenhoehe><Bruttoleistung>4000</Bruttoleistung>
    <Inbetriebnahmedatum>2025-02-01</Inbetriebnahmedatum>
  </EinheitWind>
  <EinheitWind>
    <EinheitBetriebsstatus>38</EinheitBetriebsstatus>
    <Breitengrad>52.1</Breitengrad><Laengengrad>10.1</Laengengrad>
    <Nabenhoehe>100</Nabenhoehe><Bruttoleistung>3000</Bruttoleistung>
    <Inbetriebnahmedatum>2010-05-01</Inbetriebnahmedatum>
    <DatumEndgueltigeStilllegung>2025-03-15</DatumEndgueltigeStilllegung>
  </EinheitWind>
  <EinheitWind>
    <EinheitBetriebsstatus>37</EinheitBetriebsstatus>
    <Breitengrad>52.2</Breitengrad><Laengengrad>10.2</Laengengrad>
    <Nabenhoehe>90</Nabenhoehe><Bruttoleistung>2500</Bruttoleistung>
    <Inbetriebnahmedatum>2012-01-01</Inbetriebnahmedatum>
    <DatumBeginnVoruebergehendeStilllegung>2025-01-01</DatumBeginnVoruebergehendeStilllegung>
  </EinheitWind>
  <EinheitWind>
    <EinheitBetriebsstatus>38</EinheitBetriebsstatus>
    <Breitengrad>52.3</Breitengrad><Laengengrad>10.3</Laengengrad>
    <Nabenhoehe>80</Nabenhoehe><Bruttoleistung>2000</Bruttoleistung>
    <Inbetriebnahmedatum>2005-01-01</Inbetriebnahmedatum>
    <DatumEndgueltigeStilllegung>2024-12-31</DatumEndgueltigeStilllegung>
  </EinheitWind>
  <EinheitWind>
    <EinheitBetriebsstatus>38</EinheitBetriebsstatus>
    <Breitengrad>52.4</Breitengrad><Laengengrad>10.4</Laengengrad>
    <Nabenhoehe>70</Nabenhoehe><Bruttoleistung>1500</Bruttoleistung>
    <Inbetriebnahmedatum>2004-01-01</Inbetriebnahmedatum>
  </EinheitWind>
</EinheitenWind>''')
            # OSM contains turbines at 52.1 (2025-dismantled: must pair and inherit 100m/3000kW,
            # not get dropped!), 52.3 (2024-dismantled: must drop), and 52.4 (undated shutdown: kept).
            osm = io.StringIO(
                osm_feature(1, 52.1, 10.1)
                + osm_feature(2, 52.3, 10.3)
                + osm_feature(3, 52.4, 10.4)
            )
            out_path = f'{root}/out.txt'
            turbines.main(root, out_path, {'mastr'}, as_of=CUTOFF, stdin=osm)
            with open(out_path, encoding='utf-8') as handle:
                lines = [line.strip().split('\t') for line in handle]
            self.assertEqual(lines, [
                ['52.100000', '10.100000', '100', '3000', 'osm'],
                ['52.200000', '10.200000', '90', '2500', 'mastr'],
                ['52.400000', '10.400000', '0', '0', 'osm'],
            ])

    def test_nrcan_filters_commissioning_ranges_and_dated_or_undated_decommission_notes(self):
        fake_rows = [
            ({'Commissioning': '2023/2024', 'Notes': '', 'Latitude': 44.0, 'Longitude': -75.0,
              'Hub Height (m)': 80, 'Turbine Rated Capacity (kW)': 2000}, None, None),
            ({'Commissioning': '2024/2025', 'Notes': '', 'Latitude': 45.0, 'Longitude': -75.0,
              'Hub Height (m)': 80, 'Turbine Rated Capacity (kW)': 2000}, None, None),
            ({'Commissioning': '2025', 'Notes': '', 'Latitude': 46.0, 'Longitude': -75.0,
              'Hub Height (m)': 90, 'Turbine Rated Capacity (kW)': 3000}, None, None),
            ({'Commissioning': '2010', 'Notes': 'Turbine decommissioned in 2025, original capacity 2 MW',
              'Latitude': 47.0, 'Longitude': -75.0, 'Hub Height (m)': 80,
              'Turbine Rated Capacity (kW)': 2000}, None, None),
            ({'Commissioning': '2010', 'Notes': 'Turbine decommissioned 2022',
              'Latitude': 48.0, 'Longitude': -75.0, 'Hub Height (m)': 80,
              'Turbine Rated Capacity (kW)': 2000}, None, None),
            ({'Commissioning': '2010', 'Notes': 'Turbine decommissioned, original capacity was 750 kW',
              'Latitude': 49.0, 'Longitude': -75.0, 'Hub Height (m)': 50,
              'Turbine Rated Capacity (kW)': 750}, None, None),
        ]
        with mock.patch.object(turbines, 'layer_rows', return_value=fake_rows):
            rows = list(turbines.nrcan('/unused', as_of=CUTOFF))
        self.assertEqual([(r[0], r[1]) for r in rows], [
            ('standing', 44.0),
            ('standing', 47.0),
            ('dismantled', 48.0),
        ])

    def test_ens_filters_connection_date_and_restores_post_cutoff_deregistration(self):
        def make_row(cols):
            return ({f'Field{i}': v for i, v in enumerate(cols)}, None, None)

        pad = [make_row([''] * 16)] * 3
        current = pad + [
            make_row(['', '', '2024-06-01', None, 3000, '', 90, '', '', '', '', '', '', 550000, 6200000]),
            make_row(['', '', '2025-01-15', None, 4000, '', 100, '', '', '', '', '', '', 550000, 6200000]),
            make_row(['', '', '2015-01-01', '2025-02-01', 2000, '', 80, '', '', '', '', '', '', 550000, 6200000]),
        ]
        historic = pad + [
            make_row(['', '2010-01-01', '2025-05-01', 1500, '', 70, '', '', '', '', '', '', 550000, 6200000]),
            make_row(['', '2005-01-01', '2024-11-01', 1000, '', 60, '', '', '', '', '', '', 550000, 6200000]),
            make_row(['', '2000-01-01', None, 600, '', 50, '', '', '', '', '', '', 550000, 6200000]),
        ]
        with mock.patch.object(turbines, 'layer_rows', side_effect=[current, historic]):
            rows = list(turbines.ens('/unused', as_of=CUTOFF))
        self.assertEqual([(r[0], r[4]) for r in rows], [
            ('standing', 3000.0),
            ('standing', 2000.0),
            ('standing', 1500.0),
            ('dismantled', 1000.0),
        ])

    def test_vindbrukskollen_filters_uppfort_and_skips_undated_nedmonterat_at_cutoff(self):
        fake_rows = [
            ({'STATUS': 'Uppfört', 'UPPFORT': '2024/12/01 00:00:00', 'NAVHOJD': 90, 'MAXEFFEKT': 3.0},
             (15.0, 60.0), None),
            ({'STATUS': 'Uppfört', 'UPPFORT': '2025/02', 'NAVHOJD': 110, 'MAXEFFEKT': 5.0},
             (15.1, 60.1), None),
            ({'STATUS': 'Nedmonterat', 'UPPFORT': '2000/01/01 00:00:00', 'NAVHOJD': 60, 'MAXEFFEKT': 1.0},
             (15.2, 60.2), None),
        ]
        with mock.patch.object(turbines.glob, 'glob', return_value=['/unused.gpkg']), \
                mock.patch.object(turbines, 'to_degrees', return_value=lambda x, y: (y, x)), \
                mock.patch.object(turbines, 'layer_rows', return_value=fake_rows):
            self.assertEqual(
                [(r[0], r[1]) for r in turbines.vindbrukskollen('/unused', as_of=CUTOFF)],
                [('standing', 60.0)],
            )
            self.assertEqual(
                [(r[0], r[1]) for r in turbines.vindbrukskollen('/unused')],
                [('standing', 60.0), ('standing', 60.1), ('dismantled', 60.2)],
            )

    def test_nve_drops_pre_cutoff_closed_osm_turbines_and_restores_post_cutoff_closures(self):
        def epoch_ms(year, month, day):
            return int(datetime.datetime(year, month, day, tzinfo=datetime.timezone.utc).timestamp() * 1000)

        self.assertIsNone(turbines.parse_epoch_ms(1e25))
        with tempfile.TemporaryDirectory() as root:
            os.makedirs(f'{root}/nve')
            with open(f'{root}/nve/parks.geojson', 'w', encoding='utf-8') as handle:
                json.dump({'features': [
                    {'properties': {'anleggsnr': 1, 'antallturbiner': 1, 'effekt_mw_idrift': 3.0,
                                    'forsteidriftdato': epoch_ms(2024, 6, 1), 'utavdriftdato': None}},
                    {'properties': {'anleggsnr': 2, 'antallturbiner': 1, 'effekt_mw_idrift': 4.0,
                                    'forsteidriftdato': epoch_ms(2025, 3, 1), 'utavdriftdato': None}},
                    {'properties': {'anleggsnr': 3, 'antallturbiner': 1, 'effekt_mw_idrift': 2.0,
                                    'forsteidriftdato': epoch_ms(2005, 1, 1), 'utavdriftdato': epoch_ms(2024, 10, 1)}},
                    {'properties': {'anleggsnr': 4, 'antallturbiner': 1, 'effekt_mw_idrift': 2.5,
                                    'forsteidriftdato': epoch_ms(2008, 1, 1), 'utavdriftdato': epoch_ms(2025, 4, 1)}},
                ]}, handle)
            with open(f'{root}/nve/turbines.geojson', 'w', encoding='utf-8') as handle:
                json.dump({'features': [
                    {'properties': {'anleggsnr': 1, 'status': 'D'}, 'geometry': {'type': 'Point', 'coordinates': [11.0, 61.0]}},
                    {'properties': {'anleggsnr': 2, 'status': 'D'}, 'geometry': {'type': 'Point', 'coordinates': [12.0, 62.0]}},
                    # Park 3 closed before cutoff (non-D status): must yield dismantled and drop OSM at (63, 13).
                    {'properties': {'anleggsnr': 3, 'status': 'N'}, 'geometry': {'type': 'Point', 'coordinates': [13.0, 63.0]}},
                    # Park 4 closed after cutoff (non-D status): must restore to standing and pair OSM at (64, 14).
                    {'properties': {'anleggsnr': 4, 'status': 'N'}, 'geometry': {'type': 'Point', 'coordinates': [14.0, 64.0]}},
                ]}, handle)
            osm = io.StringIO(osm_feature(1, 63.0, 13.0) + osm_feature(2, 64.0, 14.0))
            out_path = f'{root}/out.txt'
            turbines.main(root, out_path, {'nve'}, as_of=CUTOFF, stdin=osm)
            with open(out_path, encoding='utf-8') as handle:
                lines = [line.strip().split('\t') for line in handle]
            self.assertEqual(lines, [
                ['61.000000', '11.000000', '0', '3000', 'nve'],
                ['64.000000', '14.000000', '0', '2500', 'osm'],
            ])

    def test_parse_args_accepts_optional_as_of_and_rejects_bad_dates(self):
        self.assertEqual(
            turbines.parse_args(['--as-of', '2025-01-01', '/reg', '/out', 'mastr']),
            ('/reg', '/out', {'mastr'}, CUTOFF),
        )
        self.assertEqual(
            turbines.parse_args(['/reg', '/out', 'clm']),
            ('/reg', '/out', {'clm'}, None),
        )
        with self.assertRaises(ValueError):
            turbines.parse_args(['--as-of', '2025-1-1', '/reg', '/out'])


if __name__ == '__main__':
    unittest.main()
