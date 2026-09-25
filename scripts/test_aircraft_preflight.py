"""The aircraft preflight reuses the producer's window, GA receipts and native day admission."""

import unittest
from datetime import date
from pathlib import Path
from unittest.mock import patch

from aircraft_preflight import preflight_aircraft_sources
from aircraft_window import resolve_anchor, sampling_days


class Completed:
    def __init__(self, returncode=0, stdout=b'', stderr=b''):
        self.returncode, self.stdout, self.stderr = returncode, stdout, stderr


class AircraftPreflightTest(unittest.TestCase):
    def window_csv(self, anchor):
        airlines, ga_days = sampling_days(resolve_anchor(anchor, date(2026, 9, 16)))
        self.assertEqual(len(airlines), 12)
        self.assertEqual(airlines[-1].isoformat(), '2026-08-01')
        self.assertEqual(len(ga_days), 365)
        return (','.join(day.isoformat() for day in airlines),
                ','.join(day.isoformat() for day in ga_days))

    def recorder(self, ga=Completed(), cargo=Completed(), native=Completed()):
        commands = []

        def run(command, **_):
            commands.append(command)
            if 'download-adsblol.py' in str(command[1]):
                return ga
            if command[0] == 'cargo':
                return cargo
            return native

        return commands, run

    def test_complete_caches_are_admitted_with_producer_validators_and_reported_counts(self):
        airline_csv, ga_csv = self.window_csv('2026-09')
        commands, run = self.recorder(
            ga=Completed(stdout=b''.join(
                day.encode() + b'\0/received/' + day.encode() + b'/subset.tar\0'
                for day in ga_csv.split(',')[:364])))
        with patch('aircraft_preflight.shutil.which', return_value='/usr/bin/cargo'):
            admitted = preflight_aircraft_sources('/cache/airline', '/cache/ga', '2026-09', run=run)
        self.assertEqual(admitted, {'anchor': '2026-09', 'airline_days': 12,
                                    'ga_window_days': 365, 'ga_accepted_days': 364})
        self.assertTrue(commands[0][1].endswith('download-adsblol.py'))
        self.assertEqual(commands[0][2:], ['validate', '--source-root', '/cache/ga', '--days', ga_csv])
        self.assertEqual(commands[1][:2], ['cargo', 'build'])
        self.assertTrue(commands[1][4].endswith('engine/aircraft-extract/Cargo.toml'))
        self.assertEqual(commands[1][5:], ['--bin', 'aircraft-extract'])
        self.assertTrue(commands[2][0].endswith('engine/target/release/aircraft-extract'))
        self.assertEqual(commands[2][1:], ['preflight-days', '--adsb-cache', '/cache/airline',
                                           '--days', airline_csv])

    def test_incomplete_ga_acquisition_and_identity_failures_stop_the_build(self):
        _airline_csv, _ga_csv = self.window_csv('2026-09')
        commands, run = self.recorder(
            ga=Completed(1, stderr=b'selected source/cache validation failed: 2026-08-01: '
                                   b'asset remains unavailable\n'))
        with self.assertRaisesRegex(ValueError, 'GA source window incomplete.*2026-08-01'):
            preflight_aircraft_sources('/cache/airline', '/cache/ga', '2026-09', run=run)
        self.assertEqual(len(commands), 1)

    def test_missing_newest_airline_sample_stops_the_build(self):
        _airline_csv, _ga_csv = self.window_csv('2026-09')
        commands, run = self.recorder(
            native=Completed(1, stderr=b'Error: missing ADS-B day 2026-08-01: '
                                       b'/cache/airline/2026/2026-08-01\n'))
        with self.assertRaisesRegex(ValueError, 'airline source window incomplete.*2026-08-01'), \
                patch('aircraft_preflight.shutil.which', return_value='/usr/bin/cargo'):
            preflight_aircraft_sources('/cache/airline', '/cache/ga', '2026-09', run=run)
        self.assertEqual(len(commands), 3)

    def test_wrong_or_future_anchors_are_rejected_by_the_shared_selection(self):
        commands, run = self.recorder()
        for anchor in ('2026-9', '2099-01'):
            with self.subTest(anchor=anchor), self.assertRaises(ValueError):
                preflight_aircraft_sources('/cache/airline', '/cache/ga', anchor, run=run)
        self.assertEqual(commands, [])


if __name__ == '__main__':
    unittest.main()
