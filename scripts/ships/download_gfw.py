#!/usr/bin/env python3
"""Download Global Fishing Watch AIS presence hours (4Wings TIF reports, 0.01° cells) for the
whole world in 8° tiles, one report at a time, resumably: two reports per tile (large ships,
work boats), over the aircraft exposure year of the build's anchor month."""

import argparse
import hashlib
import json
from datetime import datetime, timezone
from pathlib import Path
import sqlite3
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from aircraft_window import resolve_anchor, sampling_days  # noqa: E402

API = "https://gateway.api.globalfishingwatch.org/v3"
DATASET = "public-global-presence:latest"
TILE_DEG = 8
# Web-Mercator squares end at 85.05°; AIS presence above 84° N is empty sea ice and open ocean.
LAT_MIN, LAT_MAX = -60, 84
# GFW `vessel_type` values folded into the acoustic classes of emission/ships.rs. GFW has no
# sailing / pleasure class; `other` also holds vessels without a type (measured 2026-09-17:
# the five filtered sums equal the unfiltered total in the Hamburg harbour box).
CLASS_TYPES = {
    "large": ("cargo", "tanker", "carrier", "bunker", "passenger"),
    "work": ("fishing", "support", "gear", "seismic_vessel", "other"),
}
REPORT_TIMEOUT_S = 180
LAST_REPORT_POLL_S = 10


def tiles():
    for lat in range(LAT_MIN, LAT_MAX, TILE_DEG):
        for lon in range(-180, 180, TILE_DEG):
            yield lon, lat


def tile_name(lon, lat):
    return f"lon{lon:+04d}_lat{lat:+03d}"


def polygon(lon, lat):
    lon1, lat1 = lon + TILE_DEG, min(lat + TILE_DEG, LAT_MAX)
    return {"geojson": {"type": "Polygon", "coordinates": [[[lon, lat], [lon1, lat], [lon1, lat1], [lon, lat1], [lon, lat]]]}}


def report_url(class_name, first_day, last_day, report_format="TIF"):
    query = {
        "datasets[0]": DATASET, "format": report_format, "temporal-resolution": "ENTIRE",
        "spatial-resolution": "HIGH", "spatial-aggregation": "false",
        "date-range": f"{first_day},{last_day}",
        "filters[0]": "vessel_type in (" + ",".join(f"'{t}'" for t in CLASS_TYPES[class_name]) + ")",
    }
    return f"{API}/4wings/report?" + urllib.parse.urlencode(query, quote_via=urllib.parse.quote)


class Client:
    def __init__(self, token, log=lambda message: None):
        self.token = token
        self.log = log

    def request(self, url, body=None):
        data = json.dumps(body).encode() if body is not None else None
        request = urllib.request.Request(url, data=data, method="POST" if data else "GET",
                                         headers={"Authorization": f"Bearer {self.token}", "Content-Type": "application/json",
                                                  # Cloudflare in front of the gateway refuses the default Python agent (error 1010).
                                                  "User-Agent": "quietmap-ships/1 (+https://quietmap.org)"})
        try:
            with urllib.request.urlopen(request, timeout=REPORT_TIMEOUT_S) as response:
                return response.status, response.read()
        except urllib.error.HTTPError as error:
            return error.code, error.read()
        except (urllib.error.URLError, TimeoutError, OSError) as error:
            return 0, str(error).encode()

    def report(self, url, body):
        """One 4Wings report: bytes of the zip, or b'' when the API says the tile has no data."""
        status, payload = 0, b""
        for attempt in range(6):
            status, payload = self.request(url, body)
            if status == 200:
                return payload
            if status == 404 and b"Empty data" in payload:
                return b""
            self.log(f"  attempt {attempt + 1}: HTTP {status} {payload[:160]!r}")
            if status == 503 and b"exit status 1" in payload and "format=TIF" in url and attempt >= 1:
                # The TIF renderer fails on nearly empty tiles (polar rows); the CSV report of
                # the same query lists the vessel-hours per cell and is read equivalently.
                return self.report(url.replace("format=TIF", "format=CSV"), body)
            if status == 524:
                for _ in range(60):
                    time.sleep(LAST_REPORT_POLL_S)
                    status, payload = self.request(f"{API}/4wings/last-report")
                    if status == 200:
                        return payload
                    if status == 404 and b"Empty data" in payload:
                        return b""
                    self.log(f"  last-report: HTTP {status} {payload[:120]!r}")
            if status == 429:
                time.sleep(600)
            else:
                time.sleep(30 * (attempt + 1))
        raise RuntimeError(f"report failed after retries: HTTP {status} {payload[:200]!r}")


def open_receipts(path):
    database = sqlite3.connect(path)
    database.execute("CREATE TABLE IF NOT EXISTS reports(name TEXT PRIMARY KEY, bytes INTEGER NOT NULL, "
                     "sha256 TEXT NOT NULL, seconds REAL NOT NULL, finished TEXT NOT NULL)")
    return database


def download(client, output, first_day, last_day, receipts, log):
    done = {row[0] for row in receipts.execute("SELECT name FROM reports")}
    pending = [(lon, lat, cls) for lon, lat in tiles() for cls in CLASS_TYPES]
    todo = [item for item in pending if f"{tile_name(item[0], item[1])}-{item[2]}" not in done]
    log(f"tiles {len(pending)} reports, {len(todo)} to fetch, window {first_day}..{last_day}")
    for index, (lon, lat, cls) in enumerate(todo, 1):
        name = f"{tile_name(lon, lat)}-{cls}"
        started = time.time()
        payload = client.report(report_url(cls, first_day, last_day), polygon(lon, lat))
        target = output / f"{name}.zip"
        temporary = target.with_name(f".{target.name}.tmp")
        temporary.write_bytes(payload)
        temporary.replace(target)
        seconds = time.time() - started
        receipts.execute("INSERT INTO reports VALUES(?,?,?,?,?)",
                         (name, len(payload), hashlib.sha256(payload).hexdigest(), seconds, time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())))
        receipts.commit()
        log(f"{index}/{len(todo)} {name} {len(payload)} bytes {seconds:.1f}s")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, help="directory for <tile>-<class>.zip and receipts.sqlite")
    parser.add_argument("--anchor", required=True, help="aircraft anchor month YYYY-MM; the exposure year ends the day before it")
    parser.add_argument("--token-file", default=str(Path.home() / ".config/quietmap/gfw-token"))
    args = parser.parse_args()
    output = Path(args.output)
    output.mkdir(parents=True, exist_ok=True)
    # Ships share the aircraft exposure year: the GA day list is every day of it.
    days = sampling_days(resolve_anchor(args.anchor, datetime.now(timezone.utc).date()))[1]
    first_day, last_day = days[0], days[-1]
    (output / "window.json").write_text(json.dumps({"first_day": first_day.isoformat(), "last_day": last_day.isoformat(), "days": len(days),
                                                    "dataset": DATASET, "classes": CLASS_TYPES, "tile_deg": TILE_DEG}, indent=1))
    log = lambda message: print(message, flush=True)  # noqa: E731
    client = Client(Path(args.token_file).read_text().strip(), log)
    receipts = open_receipts(output / "receipts.sqlite")
    download(client, output, first_day.isoformat(), last_day.isoformat(), receipts, log)


if __name__ == "__main__":
    main()
