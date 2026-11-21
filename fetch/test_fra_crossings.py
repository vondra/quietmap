"""Tests for FRA Form 6180.71 historical/current union fetcher and snapshot verifier."""

import csv
import io
from pathlib import Path
import tempfile
import unittest

import fra_crossings


def make_row(**overrides):
    defaults = {
        "revisiondate": "2024-06-01",
        "crossingid": "100001A",
        "crossingtypecode": "3",
        "crossingpurposecode": "1",
        "crossingpositioncode": "1",
        "crossingclosed": "No",
        "whistlebancode": "0",
        "latitude": "41.5",
        "longitude": "-93.6",
        "totaldaylightthrutrains": "6",
        "totalnighttimethrutrains": "4",
        "totalswitchingtrains": "0",
        "totaltransittrains": "0",
        "trainsperweek": "0",
        "numberpassengertrainperday": "0",
        "ltpassengertrainperday": "No",
        "numberofmaintracks": "1",
        "typeoftrainserviceids": "11",
        "maximumtimetablespeed": "40",
        "maximumspeedrangeovercrossing": "40",
        "reasondescription": "Change in Data",
        "movementsperdaycode": "2",
    }
    defaults.update(overrides)
    return [defaults[field] for field in fra_crossings.FIELDS]


def to_csv(rows):
    buf = io.StringIO()
    writer = csv.writer(buf, lineterminator="\n")
    writer.writerow(fra_crossings.FIELDS)
    writer.writerows(rows)
    return buf.getvalue()


class FraCrossingsTest(unittest.TestCase):
    def test_merge_unions_current_and_historical_keeping_closures_and_latest_revision(self):
        # 000000A: open in historical (2017), closed in current (2020) -> must stay closed.
        # 100001A: revised in 2025 (absent from current <= 2024), historical has 2024-03-15 -> kept.
        # 200002B: same date in both -> current wins.
        current = [
            make_row(crossingid="000000A", revisiondate="2020-12-10T00:00:00.000", crossingclosed="Yes"),
            make_row(crossingid="200002B", revisiondate="2024-05-01T00:00:00.000", totaldaylightthrutrains="12"),
        ]
        historical = [
            make_row(crossingid="000000A", revisiondate="2017-10-26", crossingclosed="No"),
            make_row(crossingid="100001A", revisiondate="2024-03-15", totaldaylightthrutrains="8"),
            make_row(crossingid="200002B", revisiondate="2024-05-01", totaldaylightthrutrains="9"),
        ]
        merged = fra_crossings.merge_editions(current, historical, "2024-12-31")
        self.assertEqual([r[1] for r in merged], ["000000A", "100001A", "200002B"])
        self.assertEqual(merged[0][0], "2020-12-10")
        self.assertEqual(merged[0][5], "Yes")
        self.assertEqual(merged[1][9], "8")
        self.assertEqual(merged[2][9], "12")

    def test_merge_rejects_revision_after_cutoff(self):
        with self.assertRaises(ValueError):
            fra_crossings.merge_editions(
                [make_row(crossingid="100001A", revisiondate="2025-01-01T00:00:00.000")],
                [],
                "2024-12-31",
            )

    def test_keyset_pagination_fetches_all_pages_deterministically(self):
        pages = [
            to_csv([make_row(crossingid="000001A"), make_row(crossingid="000002B")]),
            to_csv([make_row(crossingid="000003C")]),
        ]
        queries = []

        def fake_fetch(_dataset, query):
            queries.append(query)
            return pages[len(queries) - 1]

        rows = fra_crossings.fetch_dataset("vhwz-raag", "2024-12-31", False, fetcher=fake_fetch, limit=2)
        self.assertEqual([r[1] for r in rows], ["000001A", "000002B", "000003C"])
        self.assertNotIn("crossingid >", queries[0])
        self.assertIn("crossingid > '000002B'", queries[1])

    def test_check_snapshot_verifies_dates_and_headers(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "snap.csv"
            with path.open("w", encoding="utf-8", newline="") as f:
                w = csv.writer(f)
                w.writerow(fra_crossings.HEADERS)
                w.writerow(make_row(revisiondate="09/23/2026"))
            info = fra_crossings.check_snapshot(path, "2026-09-24")
            self.assertEqual(info["rows"], 1)
            self.assertEqual(info["open_at_grade"], 1)
            self.assertEqual(info["max_revision"], "2026-09-23")
            with self.assertRaises(ValueError):
                fra_crossings.check_snapshot(path, "2024-12-31")


if __name__ == "__main__":
    unittest.main()
