#!/usr/bin/env python3
"""Acquire complete monthly ADSBexchange samples with resumable SQLite source receipts and atomic TARs."""

import argparse
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
from datetime import date, datetime, timedelta, timezone
import gzip
import hashlib
import io
from itertools import islice
import json
import math
import os
from pathlib import Path
import re
import sqlite3
import tarfile
import time

import urllib3

from aircraft_window import resolve_anchor, sampling_days

BASE = 'https://samples.adsbexchange.com/'
SNAPSHOT_MINUTES = 5
HTTP = urllib3.PoolManager(maxsize=12, block=True, retries=False,
    timeout=urllib3.Timeout(connect=10, read=60), headers={'User-Agent': 'QuietMap source acquisition'})


def log(message):
    print(f'{datetime.now(timezone.utc).isoformat()} {message}', flush=True)


def fetch(path):
    failure = 'no response'
    for attempt in range(5):
        try:
            response = HTTP.request('GET', BASE + path, redirect=False)
            status, body = response.status, response.data
            headers = response.headers
            response.release_conn()
            if status in (200, 404):
                return status, body, headers.get('ETag'), headers.get('Last-Modified')
            failure = f'HTTP {status}'
        except urllib3.exceptions.HTTPError as error:
            failure = str(error)
        if attempt < 4:
            time.sleep(2 ** attempt)
    raise RuntimeError(f'{path}: unresolved after 5 attempts: {failure}')


def source_json(path, status, body, day):
    if status == 404 and path.startswith('traces/'):
        return None  # Explicit upstream absence, recorded separately from transient failures.
    if status != 200:
        raise ValueError(f'{path}: required snapshot HTTP {status}')
    obj = json.loads(gzip.decompress(body) if body.startswith(b'\x1f\x8b') else body)
    snapshot = path.startswith('readsb-hist/')
    timestamp = obj.get('now' if snapshot else 'timestamp') if isinstance(obj, dict) else None
    if not isinstance(timestamp, (int, float)) or not math.isfinite(timestamp):
        raise ValueError(f'{path}: missing or invalid source epoch')
    if snapshot:
        slot = datetime.strptime(path, 'readsb-hist/%Y/%m/%d/%H%M%SZ.json.gz').replace(tzinfo=timezone.utc).timestamp()
        if not slot - SNAPSHOT_MINUTES * 60 < timestamp <= slot:
            raise ValueError(f'{path}: source epoch is outside its snapshot interval')
    elif datetime.fromtimestamp(timestamp, timezone.utc).date() != day:
        raise ValueError(f'{path}: source epoch is outside requested day {day}')
    rows = obj.get('aircraft' if snapshot else 'trace')
    if not isinstance(rows, list):
        raise ValueError(f'{path}: missing source row array')
    day_start = datetime.combine(day, datetime.min.time(), timezone.utc)
    day_start_epoch = day_start.timestamp()
    day_end_epoch = (day_start + timedelta(days=1)).timestamp()
    for row in rows:
        if snapshot:
            if not isinstance(row, dict):
                raise ValueError(f'{path}: invalid aircraft row')
            if row.get('lat') is not None:
                identity = row.get('hex')
                if not isinstance(identity, str) or not re.fullmatch(r'~?[0-9a-fA-F]{6}', identity):
                    raise ValueError(f'{path}: positioned aircraft has invalid identity')
        else:
            if not isinstance(row, list) or len(row) < 8 or not isinstance(row[0], (int, float)):
                raise ValueError(f'{path}: invalid trace row')
            epoch = timestamp + row[0]
            # readsb writePerm selects [day start, day end), then sprintTracePoint
            # rounds offsets to 0.01 s: the final point can serialize as next midnight.
            # Preserve it to close the last segment; no next-day interval is admitted.
            if not math.isfinite(epoch) or not day_start_epoch <= epoch <= day_end_epoch:
                raise ValueError(f'{path}: trace observation is outside requested day {day}')
    return obj


def fetch_missing(database, paths, day, workers):
    cached = {row[0] for row in database.execute('SELECT path FROM responses')}
    missing = iter(path for path in paths if path not in cached)
    errors, completed = [], 0
    with ThreadPoolExecutor(max_workers=workers) as pool:
        pending = {pool.submit(fetch, path): path for path in islice(missing, workers)}
        while pending:
            done, _ = wait(pending, return_when=FIRST_COMPLETED)
            for future in done:
                path = pending.pop(future)
                try:
                    status, body, etag, modified = future.result()
                    source_json(path, status, body, day)
                    database.execute('INSERT INTO responses VALUES (?,?,?,?,?,?)',
                        (path, status, hashlib.sha256(body).hexdigest(), etag, modified, body))
                    database.commit()
                    completed += 1
                    if completed % 128 == 0:
                        log(f'{day} downloaded={completed} cached={len(cached)} last={path}')
                except Exception as error:
                    errors.append(f'{path}: {error}')
            if not errors:
                for path in islice(missing, len(done)):
                    pending[pool.submit(fetch, path)] = path
    if errors:
        raise RuntimeError(f'{day} incomplete: ' + '; '.join(errors))


def cached_response(database, path, day):
    row = database.execute('SELECT status,sha256,body FROM responses WHERE path=?', (path,)).fetchone()
    if row is None or hashlib.sha256(row[2]).hexdigest() != row[1]:
        raise ValueError(f'{path}: missing or corrupt cached source response')
    return row[0], row[2], source_json(path, row[0], row[2], day)


def download_day(day, output, workers):
    directory = output / str(day.year) / day.isoformat()
    directory.mkdir(parents=True, exist_ok=True)
    with sqlite3.connect(directory / 'progress.sqlite') as database:
        database.execute('PRAGMA journal_mode=WAL')
        database.execute('CREATE TABLE IF NOT EXISTS responses (path TEXT PRIMARY KEY, status INTEGER NOT NULL CHECK(status IN (200,404)), sha256 TEXT NOT NULL, etag TEXT, modified TEXT, body BLOB NOT NULL)')
        database.execute('CREATE TABLE IF NOT EXISTS publication (day TEXT PRIMARY KEY, snapshots INTEGER NOT NULL, traces INTEGER NOT NULL, absent INTEGER NOT NULL, bytes INTEGER NOT NULL, sha256 TEXT NOT NULL)')
        prefix = day.strftime('%Y/%m/%d/')
        snapshots = [f'readsb-hist/{prefix}{hour:02}{minute:02}00Z.json.gz'
                     for hour in range(24) for minute in range(0, 60, SNAPSHOT_MINUTES)]
        log(f'{day} phase=snapshots required={len(snapshots)}')
        fetch_missing(database, snapshots, day, workers)
        hexes = set()
        for path in snapshots:
            _, _, obj = cached_response(database, path, day)
            assert obj is not None
            hexes.update(row['hex'].lower() for row in obj['aircraft']
                if row.get('lat') is not None and not row['hex'].startswith('~'))
        if not hexes:
            raise ValueError(f'{day}: complete snapshot union contains no positioned aircraft')
        traces = [f'traces/{prefix}{identity[-2:]}/trace_full_{identity}.json' for identity in sorted(hexes)]
        log(f'{day} phase=traces required={len(traces)} snapshots={len(snapshots)}')
        fetch_missing(database, traces, day, workers)
        target = directory / 'subset.tar'
        part = directory / 'subset.tar.part'
        packed = absent = 0
        with tarfile.open(part, 'w') as archive:
            for path in traces:
                status, body, _ = cached_response(database, path, day)
                if status == 404:
                    absent += 1
                    continue
                # Deterministic bytes preserve the proven gzipped readsb entry layout.
                payload = gzip.compress(gzip.decompress(body) if body.startswith(b'\x1f\x8b') else body, mtime=0)
                item = tarfile.TarInfo('traces/' + '/'.join(path.split('/')[-2:]))
                item.size = len(payload)
                archive.addfile(item, io.BytesIO(payload))
                packed += 1
        if not packed:
            raise ValueError(f'{day}: no trace payload available; refusing an empty day')
        with part.open('rb') as stream:
            digest = hashlib.file_digest(stream, 'sha256').hexdigest()
        size = part.stat().st_size
        os.replace(part, target)
        database.execute('INSERT OR REPLACE INTO publication VALUES (?,?,?,?,?,?)',
            (day.isoformat(), len(snapshots), packed, absent, size, digest))
        database.commit()
        log(f'{day} COMPLETE snapshots={len(snapshots)} traces={packed} upstream404={absent} bytes={size} sha256={digest}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    selection = parser.add_mutually_exclusive_group(required=True)
    selection.add_argument('--days', help='Explicit comma-separated monthly sample dates')
    selection.add_argument('--last-12', action='store_true')
    parser.add_argument('--anchor', help='YYYY-MM; shared completed-month policy with --last-12')
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--workers', type=int, default=12)
    args = parser.parse_args()
    today = datetime.now(timezone.utc).date()
    try:
        if args.last_12:
            days = sampling_days(resolve_anchor(args.anchor, today))[0]
        else:
            if args.anchor:
                raise ValueError('--anchor requires --last-12')
            days = tuple(date.fromisoformat(value.strip()) for value in args.days.split(','))
        if not 1 <= args.workers <= 12 or len(set(days)) != len(days) or any(day.day != 1 or day >= today for day in days):
            raise ValueError('require 1..12 workers and unique completed first-of-month sample dates')
    except ValueError as error:
        parser.error(str(error))
    for day in days:
        download_day(day, args.out, args.workers)


if __name__ == '__main__':
    main()
