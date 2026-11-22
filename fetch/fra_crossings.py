#!/usr/bin/env python3
"""Fetch one year's FRA Form 6180.71 crossing inventory from Current (m2f8-22s6) and Historical (vhwz-raag)."""

import argparse
import csv
import datetime as dt
import hashlib
import io
import json
from pathlib import Path
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

BASE_URL = "https://data.transportation.gov/resource"
CURRENT_DATASET = "m2f8-22s6"
HISTORICAL_DATASET = "vhwz-raag"
PAGE_LIMIT = 25000
TIMEOUT_S = 120

# (CSV display header expected by fetch/rail_inputs/crossings.rs, Socrata field name)
COLUMNS = [
    ("Revision Date", "revisiondate"),
    ("Crossing ID", "crossingid"),
    ("Crossing Type Code", "crossingtypecode"),
    ("Crossing Purpose Code", "crossingpurposecode"),
    ("Crossing Position Code", "crossingpositioncode"),
    ("Crossing Closed", "crossingclosed"),
    ("Whistleban Code", "whistlebancode"),
    ("Latitude", "latitude"),
    ("Longitude", "longitude"),
    ("Total Daylight Thru Trains", "totaldaylightthrutrains"),
    ("Total Nighttime Thru Trains", "totalnighttimethrutrains"),
    ("Total Switching Trains", "totalswitchingtrains"),
    ("Total Transit Trains", "totaltransittrains"),
    ("Trains Per Week", "trainsperweek"),
    ("Number Passenger Train Per Day", "numberpassengertrainperday"),
    ("Less Than One Passenger Train Per Day", "ltpassengertrainperday"),
    ("Number Of Main Tracks", "numberofmaintracks"),
    ("Type Of Train Service IDs", "typeoftrainserviceids"),
    ("Maximum Timetable Speed", "maximumtimetablespeed"),
    ("Typical Maximum Speed Over Crossing", "maximumspeedrangeovercrossing"),
    ("Reason Description", "reasondescription"),
    ("Movements Per Day Code", "movementsperdaycode"),
]
HEADERS = [header for header, _ in COLUMNS]
FIELDS = [field for _, field in COLUMNS]


def parse_revision_date(value):
    """Normalize FRA dates (YYYY-MM-DD, ISO timestamps, or MM/DD/YYYY) to YYYY-MM-DD."""
    text = (value or "").strip()
    if not text:
        return None
    if len(text) >= 10 and text[4] == "-" and text[7] == "-":
        day = text[:10]
        dt.date.fromisoformat(day)
        return day
    if len(text) == 10 and text[2] == "/" and text[5] == "/":
        month, day, year = int(text[0:2]), int(text[3:5]), int(text[6:10])
        return dt.date(year, month, day).isoformat()
    raise ValueError(f"unsupported FRA revision date {text!r}")


def socrata_query(dataset, cutoff, is_timestamp, after_id, limit=PAGE_LIMIT):
    """Build a keyset-paginated SoQL query for the latest revision per crossing <= cutoff."""
    select = ", ".join(FIELDS)
    bound = f"{cutoff}T23:59:59.999" if is_timestamp else cutoff
    where = f"revisiondate <= '{bound}'"
    if after_id is not None:
        escaped = after_id.replace("'", "''")
        where += f" AND crossingid > '{escaped}'"
    return (
        f"SELECT DISTINCT ON (crossingid) {select} "
        f"WHERE {where} "
        f"ORDER BY crossingid, revisiondate DESC, :id DESC "
        f"LIMIT {limit}"
    )


def fetch_csv_text(dataset, query, retries=6):
    url = f"{BASE_URL}/{dataset}.csv?" + urllib.parse.urlencode({"$query": query})
    request = urllib.request.Request(
        url, headers={"User-Agent": "quietmap-fra/1 (+https://quietmap.org)"}
    )
    last_error = None
    for attempt in range(retries):
        try:
            with urllib.request.urlopen(request, timeout=TIMEOUT_S) as response:
                return response.read().decode("utf-8")
        except (urllib.error.HTTPError, urllib.error.URLError, TimeoutError, OSError) as err:
            last_error = err
            wait = 10 * (attempt + 1)
            print(
                f"fra_crossings: {dataset} attempt {attempt + 1} failed ({err}); retrying in {wait}s",
                file=sys.stderr,
                flush=True,
            )
            time.sleep(wait)
    raise RuntimeError(f"{dataset}: failed after {retries} attempts: {last_error}")


def fetch_dataset(dataset, cutoff, is_timestamp, fetcher=fetch_csv_text, limit=PAGE_LIMIT):
    """Page through every crossing's latest revision <= cutoff in one FRA Socrata table."""
    rows = []
    after_id = None
    page = 0
    while True:
        page += 1
        query = socrata_query(dataset, cutoff, is_timestamp, after_id, limit=limit)
        text = fetcher(dataset, query)
        reader = csv.reader(io.StringIO(text))
        header = next(reader, None)
        if header is None:
            raise ValueError(f"{dataset}: empty CSV response on page {page}")
        if [h.strip() for h in header] != FIELDS:
            raise ValueError(f"{dataset}: unexpected headers {header!r}")
        count = 0
        last_id = after_id
        for raw in reader:
            if len(raw) != len(FIELDS):
                raise ValueError(f"{dataset}: expected {len(FIELDS)} columns, got {len(raw)}")
            rev = parse_revision_date(raw[0])
            cid = raw[1].strip()
            if not cid or rev is None or rev > cutoff:
                raise ValueError(f"{dataset}: invalid row crossingid={cid!r} revisiondate={raw[0]!r}")
            if last_id is not None and cid <= last_id:
                raise ValueError(f"{dataset}: non-monotonic crossingid {cid!r} after {last_id!r}")
            row = [cell.strip() for cell in raw]
            row[0] = rev
            rows.append(row)
            last_id = cid
            count += 1
        print(
            f"fra_crossings: {dataset} page {page}: {count} rows (total {len(rows)})",
            file=sys.stderr,
            flush=True,
        )
        if count < limit:
            break
        after_id = last_id
    return rows


def merge_editions(current_rows, historical_rows, cutoff):
    """Union Current and Historical <= cutoff, taking the latest revision per Crossing ID."""
    best = {}
    for priority, source in ((0, historical_rows), (1, current_rows)):
        for row in source:
            rev = parse_revision_date(row[0])
            cid = row[1].strip()
            if not cid or rev is None or rev > cutoff:
                raise ValueError(f"row outside cutoff {cutoff}: id={cid!r} rev={row[0]!r}")
            normalized = [cell.strip() for cell in row]
            normalized[0] = rev
            key = (rev, priority)
            existing = best.get(cid)
            if existing is None or key > existing[0]:
                best[cid] = (key, normalized)
    return [best[cid][1] for cid in sorted(best)]


def digest(path):
    with path.open("rb") as handle:
        return hashlib.file_digest(handle, "sha256").hexdigest()


def write_normalized(rows, out_path):
    out_path.parent.mkdir(parents=True, exist_ok=True)
    temporary = out_path.with_name(out_path.name + ".tmp")
    with temporary.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.writer(handle, lineterminator="\n")
        writer.writerow(HEADERS)
        writer.writerows(rows)
    temporary.rename(out_path)
    return digest(out_path)


def check_snapshot(path, cutoff):
    """Verify an already-admitted FRA CSV snapshot has every required column and revision <= cutoff."""
    rows = 0
    open_at_grade = 0
    max_rev = ""
    with path.open("r", encoding="utf-8", errors="replace", newline="") as handle:
        reader = csv.reader(handle)
        raw_headers = [h.strip() for h in next(reader)]
        index = {name: i for i, name in enumerate(raw_headers)}
        missing = [h for h in HEADERS if h not in index]
        if missing:
            raise ValueError(f"{path}: missing required columns {missing}")
        i_rev = index["Revision Date"]
        i_closed = index["Crossing Closed"]
        i_pos = index["Crossing Position Code"]
        for row in reader:
            rows += 1
            rev = parse_revision_date(row[i_rev])
            if rev is not None:
                if rev > cutoff:
                    raise ValueError(f"{path}: row {rows + 1} revision {rev} exceeds cutoff {cutoff}")
                if rev > max_rev:
                    max_rev = rev
            if row[i_closed].strip() == "No" and row[i_pos].strip() == "1":
                open_at_grade += 1
    return {
        "sha256": digest(path),
        "cutoff": cutoff,
        "max_revision": max_rev,
        "rows": rows,
        "open_at_grade": open_at_grade,
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cutoff", required=True, help="Inclusive YYYY-MM-DD cutoff date")
    parser.add_argument("--out", type=Path, help="Output CSV path when fetching Current + Historical")
    parser.add_argument("--check-snapshot", type=Path, help="Verify an existing raw FRA CSV snapshot")
    args = parser.parse_args(argv)
    dt.date.fromisoformat(args.cutoff)
    if args.check_snapshot is not None:
        print(json.dumps(check_snapshot(args.check_snapshot, args.cutoff), indent=2))
        return 0
    if args.out is None:
        parser.error("either --out or --check-snapshot is required")
    current = fetch_dataset(CURRENT_DATASET, args.cutoff, is_timestamp=True)
    historical = fetch_dataset(HISTORICAL_DATASET, args.cutoff, is_timestamp=False)
    merged = merge_editions(current, historical, args.cutoff)
    sha256 = write_normalized(merged, args.out)
    open_at_grade = sum(1 for row in merged if row[5] == "No" and row[4] == "1")
    summary = {
        "sha256": sha256,
        "cutoff": args.cutoff,
        "rows": len(merged),
        "open_at_grade": open_at_grade,
        "current_rows": len(current),
        "historical_rows": len(historical),
    }
    print(json.dumps(summary, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
