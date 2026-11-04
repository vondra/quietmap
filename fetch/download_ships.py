#!/usr/bin/env python3
"""Download manifest-selected GFW reports with verified metadata, resumable receipts and an inventory."""

import argparse
import hashlib
import io
import json
from pathlib import Path
import sqlite3
import time
import urllib.error
import urllib.parse
import urllib.request
import zipfile

API = "https://gateway.api.globalfishingwatch.org/v3"
REPORT_TIMEOUT_S = 180
LAST_REPORT_POLL_S = 10


def tiles():
    west, south, east, north = BOUNDS
    for lat in range(south, north, TILE_DEG):
        for lon in range(west, east, TILE_DEG):
            yield lon, lat


def tile_name(lon, lat):
    return f"lon{lon:+04d}_lat{lat:+03d}"


def polygon(lon, lat):
    lon1, lat1 = lon + TILE_DEG, min(lat + TILE_DEG, BOUNDS[3])
    return {"geojson": {"type": "Polygon", "coordinates": [[[lon, lat], [lon1, lat], [lon1, lat1], [lon, lat1], [lon, lat]]]}}


def report_url(class_name, first_day, end_exclusive, report_format="TIF"):
    query = {
        "datasets[0]": DATASET, "format": report_format, "temporal-resolution": "ENTIRE",
        "spatial-resolution": "HIGH", "spatial-aggregation": "false",
        "date-range": f"{first_day},{end_exclusive}",
        "filters[0]": "vessel_type in (" + ",".join(f"'{t}'" for t in CLASS_TYPES[class_name]) + ")",
    }
    return f"{API}/4wings/report?" + urllib.parse.urlencode(query, quote_via=urllib.parse.quote)


def validate_report(payload, url, body):
    """The publisher embeds the executed filter and polygon; reject a wrong last-report response."""
    query = urllib.parse.parse_qs(urllib.parse.urlsplit(url).query)
    first, end = query["date-range"][0].split(",")
    prefix = "readme_" if query["format"][0] == "CSV" else ""
    member = "layer-activity-data-0/" + prefix + query["datasets[0]"][0].replace(":", "-") + ".md"
    with zipfile.ZipFile(io.BytesIO(payload)) as archive:
        metadata = archive.read(member).decode()
        geometry = json.loads(archive.read("layer-reference-geometry/geometry.geojson"))
    for clause in (query["filters[0]"][0], f"timestamp >= parseDateTimeBestEffort('{first}')",
                   f"timestamp < parseDateTimeBestEffort('{end}')"):
        if clause not in metadata:
            raise ValueError("Report metadata differs from its requested classes or half-open window")
    if geometry["features"][0]["geometry"] != body["geojson"]:
        raise ValueError("Report geometry differs from the requested tile")


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
                validate_report(payload, url, body)
                return payload
            if status == 404 and b"Empty data" in payload:
                return b""
            self.log(f"  attempt {attempt + 1}: HTTP {status} {payload[:160]!r}")
            if status == 503 and b"exit status 1" in payload and "format=TIF" in url and attempt >= 1:
                # Single-cell TIFF exports fail; grouped CSV works even when the publisher's
                # ungrouped CSV service is unavailable. The reader sums flags within each cell.
                return self.report(url.replace("format=TIF", "format=CSV") + "&group-by=FLAG", body)
            if status == 524:
                for _ in range(60):
                    time.sleep(LAST_REPORT_POLL_S)
                    status, payload = self.request(f"{API}/4wings/last-report")
                    if status == 200:
                        validate_report(payload, url, body)
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


def download(client, output, first_day, end_exclusive, receipts, log):
    done = set()
    for name, size, sha256 in receipts.execute("SELECT name,bytes,sha256 FROM reports"):
        path = output / f"{name}.zip"
        if not path.is_file() or path.stat().st_size != size or digest(path) != sha256:
            raise ValueError(f"Report differs from its receipt: {name}")
        done.add(name)
    pending = [(lon, lat, cls) for lon, lat in tiles() for cls in CLASS_TYPES]
    todo = [item for item in pending if f"{tile_name(item[0], item[1])}-{item[2]}" not in done]
    log(f"tiles {len(pending)} reports, {len(todo)} to fetch, window [{first_day},{end_exclusive})")
    for index, (lon, lat, cls) in enumerate(todo, 1):
        name = f"{tile_name(lon, lat)}-{cls}"
        started = time.time()
        payload = client.report(report_url(cls, first_day, end_exclusive), polygon(lon, lat))
        target = output / f"{name}.zip"
        temporary = target.with_name(f".{target.name}.tmp")
        temporary.write_bytes(payload)
        temporary.replace(target)
        seconds = time.time() - started
        receipts.execute("INSERT INTO reports VALUES(?,?,?,?,?)",
                         (name, len(payload), hashlib.sha256(payload).hexdigest(), seconds, time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())))
        receipts.commit()
        log(f"{index}/{len(todo)} {name} {len(payload)} bytes {seconds:.1f}s")


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def main():
    global DATASET, TILE_DEG, BOUNDS, CLASS_TYPES
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--year", required=True, type=Path, help="year manifest")
    parser.add_argument("--sources", required=True, type=Path, help="root holding YEAR/ID")
    parser.add_argument("--input-id", required=True)
    parser.add_argument("--token-file", required=True, type=Path)
    args = parser.parse_args()
    manifest = json.loads(args.year.read_text())
    item = next(i for i in manifest["inputs"] if i["id"] == args.input_id and i["kind"] == "prepare-ships")
    spec = item["conversion"]
    if spec["format"] != "presence-reports":
        raise ValueError("Expected presence reports")
    DATASET = spec["dataset"]
    acquisition = spec["acquisition"]
    TILE_DEG, BOUNDS, CLASS_TYPES = acquisition["tile_degrees"], acquisition["bounds"], acquisition["classes"]
    from datetime import date, datetime, timedelta, timezone
    start, end = (date.fromisoformat(manifest["flows"][key]) for key in ("start", "end"))
    if start >= end or end > datetime.now(timezone.utc).date():
        raise ValueError("The entire observation window must have finished in UTC")
    last = end - timedelta(days=1)
    if item["observed"] != {"start": start.isoformat(), "end": last.isoformat()}:
        raise ValueError("Presence observations must match the manifest flow window")
    output = args.sources / manifest["year"] / item["id"]
    output.mkdir(parents=True, exist_ok=True)
    window = {"first_day": start.isoformat(), "last_day": last.isoformat(), "end_exclusive": end.isoformat(), "days": (end-start).days,
              "dataset": DATASET, "classes": CLASS_TYPES, "tile_deg": TILE_DEG}
    window_path = output / spec["window"]
    if window_path.exists():
        if json.loads(window_path.read_text()) != window:
            raise ValueError("Refusing a different report window in an existing input")
    else:
        window_path.write_text(json.dumps(window, indent=1))
    log = lambda message: print(message, flush=True)
    receipts = open_receipts(output / "receipts.sqlite")
    client = Client(args.token_file.read_text().strip(), log)
    download(client, output, start.isoformat(), end.isoformat(), receipts, log)
    objects = {name: {"path": name + ".zip", "bytes": size, "sha256": sha}
               for name, size, sha in receipts.execute("SELECT name,bytes,sha256 FROM reports")}
    receipts.close()
    reports = [{"bounds": [lon, lat, lon+TILE_DEG, lat+TILE_DEG],
                **{cls: objects[f"{tile_name(lon,lat)}-{cls}"] for cls in CLASS_TYPES}}
               for lon, lat in tiles()]
    inventory = output / spec["inventory"]["path"]
    content = json.dumps({"window_sha256": digest(window_path), "reports": reports}, indent=2) + "\n"
    if inventory.exists():
        if inventory.read_text() != content:
            raise ValueError("Refusing to replace a different report inventory")
    else:
        temporary = inventory.with_suffix(".tmp")
        temporary.write_text(content)
        temporary.replace(inventory)
    print(json.dumps({"path": spec["inventory"]["path"], "sha256": digest(inventory)}))


if __name__ == "__main__":
    main()
