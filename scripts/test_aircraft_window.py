"""The actual runner must pass aligned calendar windows, never the archive inventory."""

from datetime import date, timedelta
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

from aircraft_window import resolve_anchor, sampling_days


class AircraftWindowTests(unittest.TestCase):
    def test_calendar_window_covers_leap_days_and_year_boundaries(self):
        for year in range(2023, 2027):
            for month in range(1, 13):
                anchor = date(year, month, 1)
                airlines, ga = sampling_days(anchor)
                self.assertEqual((len(set(airlines)), len(set(ga))), (12, 365))
                self.assertEqual(airlines[-1], anchor)
                self.assertEqual(ga[-1], anchor)
                self.assertEqual(ga[0], anchor - timedelta(days=364))
                self.assertTrue(set(airlines) <= set(ga))
                self.assertTrue(all(day.day == 1 for day in airlines))
                self.assertEqual([day.year * 12 + day.month for day in airlines],
                                 list(range(year * 12 + month - 11, year * 12 + month + 1)))
        airlines, ga = sampling_days(date(2026, 9, 1))
        self.assertEqual((airlines[0], airlines[-1]), (date(2025, 10, 1), date(2026, 9, 1)))
        self.assertEqual((ga[0], ga[-1]), (date(2025, 9, 2), date(2026, 9, 1)))
        self.assertIn(date(2024, 2, 29), sampling_days(date(2024, 3, 1))[1])

    def test_anchor_requires_a_completed_utc_sample_day(self):
        for today, expected in [(date(2026, 9, 1), date(2026, 8, 1)),
                                (date(2026, 9, 2), date(2026, 9, 1)),
                                (date(2026, 1, 1), date(2025, 12, 1))]:
            self.assertEqual(resolve_anchor(None, today), expected)
        self.assertEqual(resolve_anchor("2024-03", date(2026, 9, 5)), date(2024, 3, 1))
        for month in ["2026-09", "2026-10", "2026-13", "2026-9", "2026-09-01"]:
            with self.assertRaises(ValueError):
                resolve_anchor(month, date(2026, 9, 1))
        with self.assertRaises(ValueError):
            sampling_days(date(2026, 9, 2))

    def runner_fixture(self, root):
        scripts = root / "scripts"
        scripts.mkdir()
        for name in ["run-aircraft-extract.sh", "aircraft_window.py"]:
            shutil.copyfile(Path(__file__).with_name(name), scripts / name)
        binary = root / "engine/target/release/aircraft-extract"
        binary.parent.mkdir(parents=True)
        binary.write_text("#!/usr/bin/env python3\nimport json,os,sys\n"
                          "with open(os.environ['RECORDED_CALLS'],'a') as output:\n"
                          "    output.write(json.dumps(sys.argv[1:])+'\\n')\n")
        binary.chmod(0o755)
        commands = root / "commands"
        commands.mkdir()
        cargo = commands / "cargo"
        cargo.write_text("#!/bin/sh\nexit 0\n")
        cargo.chmod(0o755)
        environment = {key: value for key, value in os.environ.items()
                       if key not in {"DAYS", "AIRLINE_DAYS", "GA_DAYS", "FROM_STAGE", "SCOPE_BBOX"}}
        environment.update(HYBRID="1", AIRCRAFT_ANCHOR="2024-03", MEMMAX="",
                           AIRLINE_FEED="adsbexchange", FEED="adsblol",
                           PREPARED_YEAR_DIR=str(root / "prepared"), PREPARED_DIR=str(root / "prepared"),
                           AIRLINE_CACHE=str(root / "airlines"), GA_CACHE=str(root / "ga"),
                           WORK_DIR=str(root / "work"), LOG_DIR=str(root / "logs"),
                           RECORDED_CALLS=str(root / "calls.jsonl"),
                           PATH=f"{commands}:{os.environ['PATH']}")
        return scripts / "run-aircraft-extract.sh", environment

    def test_real_runner_passes_both_derived_windows_without_reading_archive_dates(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runner, environment = self.runner_fixture(root)
            result = subprocess.run(["bash", str(runner)], env=environment, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            calls = [json.loads(line) for line in (root / "calls.jsonl").read_text().splitlines()]
            self.assertEqual([call[0] for call in calls], ["run-all", "run-all", "run-all", "audit"])
            airlines, ga = sampling_days(date(2024, 3, 1))
            for call, days in zip(calls[:3], [airlines, ga, airlines]):
                self.assertEqual(call[call.index("--days") + 1].split(","),
                                 [day.isoformat() for day in days])
            self.assertEqual(calls[0][calls[0].index("--class-filter") + 1], "non-ga")
            self.assertEqual(calls[1][calls[1].index("--class-filter") + 1], "ga")

    def test_manual_hybrid_day_lists_are_rejected_before_build_or_extract(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runner, environment = self.runner_fixture(root)
            for name in ["DAYS", "AIRLINE_DAYS", "GA_DAYS"]:
                result = subprocess.run(["bash", str(runner)], env={**environment, name: "2024-01-01"},
                                        capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("one AIRCRAFT_ANCHOR", result.stdout)
                self.assertFalse((root / "calls.jsonl").exists())
                self.assertFalse((root / "logs").exists())


if __name__ == "__main__":
    unittest.main()
