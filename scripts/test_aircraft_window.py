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
    def test_window_is_the_half_open_year_before_the_anchor(self):
        for year in range(2023, 2028):
            for month in range(1, 13):
                anchor = date(year, month, 1)
                baseline, increment = sampling_days(anchor)
                first_day = date(year - 1, month, 1)
                self.assertEqual(baseline, tuple(first_day + timedelta(days=offset)
                                                 for offset in range((anchor - first_day).days)))
                self.assertEqual(increment, tuple(day for day in baseline if day.day == 1))
                self.assertEqual(len(increment), 12)
        baseline, increment = sampling_days(date(2027, 1, 1))
        self.assertEqual((baseline[0], baseline[-1], len(baseline)), (date(2026, 1, 1), date(2026, 12, 31), 365))
        self.assertEqual(increment, tuple(date(2026, month, 1) for month in range(1, 13)))
        baseline, increment = sampling_days(date(2026, 10, 1))
        self.assertEqual((baseline[0], baseline[-1]), (date(2025, 10, 1), date(2026, 9, 30)))
        self.assertEqual((increment[0], increment[-1]), (date(2025, 10, 1), date(2026, 9, 1)))
        self.assertEqual(len(sampling_days(date(2025, 1, 1)).baseline), 366)
        self.assertIn(date(2024, 2, 29), sampling_days(date(2024, 3, 1)).baseline)

    def test_anchor_requires_the_last_window_day_to_have_finished(self):
        for today, expected in [(date(2026, 9, 1), date(2026, 9, 1)),
                                (date(2026, 9, 24), date(2026, 9, 1)),
                                (date(2027, 1, 1), date(2027, 1, 1))]:
            self.assertEqual(resolve_anchor(None, today), expected)
        self.assertEqual(resolve_anchor("2026-09", date(2026, 9, 1)), date(2026, 9, 1))
        self.assertEqual(resolve_anchor("2024-03", date(2026, 9, 5)), date(2024, 3, 1))
        for month in ["2026-10", "2026-13", "2026-9", "2026-09-01"]:
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
                       if key not in {"DAYS", "INCREMENT_DAYS", "FROM_STAGE", "UNTIL_STAGE", "SCOPE_BBOX"}}
        environment.update(AIRCRAFT_ANCHOR="2024-03", MEMMAX="",
                           PREPARED_YEAR_DIR=str(root / "prepared"), PREPARED_DIR=str(root / "prepared"),
                           ADSB_CACHE=str(root / "adsblol"), SECONDARY_ADSB_CACHE=str(root / "adsbx"),
                           WORK_DIR=str(root / "work"), LOG_DIR=str(root / "logs"),
                           RECORDED_CALLS=str(root / "calls.jsonl"),
                           PATH=f"{commands}:{os.environ['PATH']}")
        return scripts / "run-aircraft-extract.sh", environment

    def calls(self, root):
        return [json.loads(line) for line in (root / "calls.jsonl").read_text().splitlines()]

    def test_real_runner_passes_the_anchor_year_to_one_union_pass(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runner, environment = self.runner_fixture(root)
            result = subprocess.run(["bash", str(runner), "--from-stage", "stage2a"], env=environment,
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            calls = self.calls(root)
            self.assertEqual([call[0] for call in calls], ["run-all", "audit"])
            run = calls[0]
            baseline, increment = sampling_days(date(2024, 3, 1))
            self.assertEqual(len(baseline), 366, "leap year 2023-03..2024-02")
            self.assertEqual(run[run.index("--days") + 1].split(","), [day.isoformat() for day in baseline])
            self.assertEqual(run[run.index("--increment-days") + 1].split(","),
                             [day.isoformat() for day in increment])
            self.assertEqual(run[run.index("--adsb-cache") + 1], environment["ADSB_CACHE"])
            self.assertEqual(run[run.index("--secondary-adsb-cache") + 1], environment["SECONDARY_ADSB_CACHE"])
            self.assertEqual(run[run.index("--from-stage") + 1], "stage2a")
            self.assertEqual(calls[1][-1], str(root / "work/segments_by_square"))

    def test_a_primary_only_run_reads_no_increment_days(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runner, environment = self.runner_fixture(root)
            del environment["SECONDARY_ADSB_CACHE"]
            result = subprocess.run(["bash", str(runner)], env=environment, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            run = self.calls(root)[0]
            self.assertNotIn("--increment-days", run)
            self.assertNotIn("--secondary-adsb-cache", run)

    def test_manual_day_lists_are_rejected_beside_an_anchor(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runner, environment = self.runner_fixture(root)
            for name in ["DAYS", "INCREMENT_DAYS"]:
                result = subprocess.run(["bash", str(runner)], env={**environment, name: "2024-01-01"},
                                        capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("one AIRCRAFT_ANCHOR", result.stdout)
                self.assertFalse((root / "calls.jsonl").exists())
                self.assertFalse((root / "logs").exists())
            del environment["AIRCRAFT_ANCHOR"]
            del environment["SECONDARY_ADSB_CACHE"]
            result = subprocess.run(["bash", str(runner)],
                                    env={**environment, "DAYS": "2024-01-01", "INCREMENT_DAYS": "2024-01-01"},
                                    capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("needs SECONDARY_ADSB_CACHE", result.stdout)


if __name__ == "__main__":
    unittest.main()
