#!/usr/bin/env python3
"""Acquire publisher-identified daily ADSB.lol exports without overwriting retained archives."""

import argparse
import bisect
from contextlib import ExitStack, closing
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
from datetime import date, datetime, timezone
import hashlib
from itertools import islice
import json
import os
from pathlib import Path
import re
import shutil
import sqlite3
import stat
import sys
import tempfile
import tarfile
import time
import urllib.request

from aircraft_window import resolve_anchor, sampling_days

API = 'https://api.github.com/repos/adsblol/globe_history_'
RAW = 'https://raw.githubusercontent.com/adsblol/globe_history_'
STAT_FIELDS = ('st_dev', 'st_ino', 'st_size', 'st_mtime_ns', 'st_ctime_ns')


def log(message):
    print(f'{datetime.now(timezone.utc).isoformat()} {message}', flush=True)


def identity(path):
    value = path.lstat()
    if not stat.S_ISREG(value.st_mode):
        raise ValueError(f'not a regular source file: {path}')
    return tuple(getattr(value, key) for key in STAT_FIELDS)


def request(url):
    return urllib.request.urlopen(urllib.request.Request(url, headers={
        'User-Agent': 'QuietMap source acquisition'}), timeout=60)


def response(database, url):
    cached = database.execute('SELECT body FROM responses WHERE url=?', (url,)).fetchone()
    if cached:
        return cached[0]
    with request(url) as remote:
        body = remote.read()
    database.execute('INSERT INTO responses VALUES (?,?)', (url, body))
    return body


def preferred_releases(database, days, preserved_response=None):
    fetch = preserved_response if preserved_response is not None else lambda url: response(database, url)
    preferred, releases = {}, {}
    published = {}
    years = sorted({day.year for day in days})
    # A December release may be published in January's repository. The original
    # calendar-year repository wins; rollover supplies only otherwise absent days.
    for year in range(years[0], years[-1] + 2):
        if year > years[-1] and all(day.isoformat() in preferred for day in days):
            break
        commit = json.loads(fetch(f'{API}{year}/commits/main'))
        lines = fetch(f'{RAW}{year}/{commit["sha"]}/PREFERRED_RELEASES.txt').decode()
        for line in lines.splitlines():
            urls = line.split(',')
            match = re.search(r'/v(\d{4}\.\d{2}\.\d{2})-', urls[0])
            if not match:
                raise ValueError('invalid official preferred release date')
            day = date.fromisoformat(match[1].replace('.', '-'))
            if day in days and day.isoformat() not in preferred:
                if day.year < year and day.isoformat() in published.get(day.year, set()):
                    raise ValueError(f'{day}: original repository has a release but no preferred export')
                preferred[day.isoformat()] = urls
        page = 1
        while True:
            batch = json.loads(fetch(f'{API}{year}/releases?per_page=100&page={page}'))
            if not batch:
                break
            for release in batch:
                published.setdefault(year, set()).add(release['tag_name'][1:11].replace('.', '-'))
                for asset in release['assets']:
                    releases[asset['browser_download_url']] = (release, asset)
            if len(batch) < 100:
                break
            page += 1
    missing = sorted(day.isoformat() for day in days if day.isoformat() not in preferred)
    if missing:
        raise ValueError(f'official release missing for requested days: {missing}')
    return preferred, releases


def release_assets(day, urls, releases):
    chosen = [releases[url] for url in urls]
    tags = {release['tag_name'] for release, _ in chosen}
    if len(tags) != 1 or len(set(urls)) != len(urls):
        raise ValueError(f'{day}: duplicate assets or multiple preferred exports')
    tag = tags.pop()
    if not re.fullmatch(r'v' + day.replace('-', r'\.') + r'-planes-readsb-(prod|staging|mlatonly)-\d+(tmp)?', tag):
        raise ValueError(f'{day}: invalid release tag {tag}')
    release = chosen[0][0]
    if release['draft'] or set(urls) != {asset['browser_download_url'] for asset in release['assets']}:
        raise ValueError(f'{day}: draft or incomplete official asset list')
    assets = []
    for _, asset in chosen:
        name, digest = asset['name'], asset['digest']
        if (asset['state'] != 'uploaded' or not isinstance(asset['size'], int) or asset['size'] <= 0
                or not isinstance(digest, str) or not re.fullmatch(r'sha256:[0-9a-f]{64}', digest)
                or not re.fullmatch(re.escape(tag) + r'\.tar(?:\.[a-z]{2})?', name)
                or not re.fullmatch(r'https://github\.com/adsblol/globe_history_\d{4}/releases/download/'
                                    + re.escape(tag + '/' + name), asset['browser_download_url'])):
            raise ValueError(f'{day}: invalid published asset identity {name}')
        assets.append((day, name, asset['browser_download_url'], asset['size'], digest[7:], tag))
    suffixes = sorted(name.removeprefix(tag + '.tar') for _, name, *_ in assets if name != tag + '.tar')
    expected = [f'.{chr(97+i//26)}{chr(97+i%26)}' for i in range(len(suffixes))]
    if suffixes != expected or not assets:
        raise ValueError(f'{day}: missing split archive part')
    return assets


def open_catalog(output, days):
    output.mkdir(parents=True, exist_ok=True)
    database = sqlite3.connect(output / 'catalog.sqlite')
    database.execute('CREATE TABLE IF NOT EXISTS responses (url TEXT PRIMARY KEY, body BLOB NOT NULL)')
    database.execute('CREATE TABLE IF NOT EXISTS window (day TEXT PRIMARY KEY)')
    database.execute('CREATE TABLE IF NOT EXISTS assets (day TEXT, name TEXT, url TEXT UNIQUE, size INTEGER, sha256 TEXT, tag TEXT, PRIMARY KEY(day,name))')
    database.execute('CREATE TABLE IF NOT EXISTS verified (path TEXT PRIMARY KEY, sha256 TEXT, dev INTEGER, ino INTEGER, size INTEGER, mtime_ns INTEGER, ctime_ns INTEGER)')
    requested = tuple(day.isoformat() for day in days)
    existing = tuple(row[0] for row in database.execute('SELECT day FROM window ORDER BY day'))
    if existing and existing != requested:
        raise ValueError('catalog belongs to a different observation window; use a new output directory')
    with database:
        if not existing:
            # Orphan metadata from an interrupted bootstrap must not pin a stale catalog.
            database.execute('DELETE FROM responses')
        preferred, releases = preferred_releases(database, set(days))
        assets = [asset for day, urls in preferred.items() for asset in release_assets(day, urls, releases)]
        if existing:
            stored = database.execute('SELECT day,name,url,size,sha256,tag FROM assets ORDER BY day,name').fetchall()
            if sorted(assets) != stored:
                raise ValueError('catalog asset rows differ from preserved official responses')
        else:
            database.executemany('INSERT INTO assets VALUES (?,?,?,?,?,?)', assets)
            database.executemany('INSERT INTO window VALUES (?)', [(day,) for day in requested])
    return database


def import_verified(database, receipt):
    if receipt is None:
        return
    # This is an independently verified local checksum receipt, not a new hash
    # assertion inferred from a file's presence. Changed identities are rejected.
    with sqlite3.connect(f'{receipt.resolve().as_uri()}?mode=ro', uri=True) as source:
        rows = source.execute('SELECT path,sha256,dev,ino,size,mtime_ns,ctime_ns FROM verified').fetchall()
    for path, digest, *expected in rows:
        if not re.fullmatch(r'[0-9a-f]{64}', digest) or identity(Path(path)) != tuple(expected):
            raise ValueError(f'changed independently verified source: {path}')
    with database:
        database.executemany('INSERT OR REPLACE INTO verified VALUES (?,?,?,?,?,?,?)', rows)


def verified_asset(database, asset):
    for row in database.execute('SELECT path,dev,ino,size,mtime_ns,ctime_ns FROM verified WHERE sha256=? AND size=?', (asset[4], asset[3])):
        if identity(Path(row[0])) != tuple(row[1:]):
            raise ValueError(f'changed verified archive: {row[0]}')
        return row[0]
    return None


def asset_target_path(asset, output):
    day, name, *_ = asset
    return output / day[:4] / day / name


def acquire(asset, output, reserve_bytes):
    _, name, url, size, digest, _ = asset
    target = asset_target_path(asset, output)
    directory = target.parent
    directory.mkdir(parents=True, exist_ok=True)
    if target.exists() or target.is_symlink():
        before = identity(target)
        with target.open('rb') as stream:
            actual = hashlib.file_digest(stream, 'sha256').hexdigest()
        if before != identity(target) or before[2] != size or actual != digest:
            raise ValueError(f'existing archive differs from publisher; retained unchanged: {target}')
        return (str(target), digest, *before)
    for attempt in range(3):
        temporary = None
        try:
            with tempfile.NamedTemporaryFile(dir=directory, prefix='.download-', delete=False) as stream:
                temporary = Path(stream.name)
                computed, received = hashlib.sha256(), 0
                with request(url) as remote:
                    while chunk := remote.read(4 * 1024 * 1024):
                        if shutil.disk_usage(output).free - len(chunk) < reserve_bytes:
                            raise ValueError('download stopped at free-space reserve')
                        received += len(chunk)
                        if received > size:
                            raise ValueError(f'publisher size exceeded: {url}')
                        stream.write(chunk)
                        computed.update(chunk)
                if received != size or computed.hexdigest() != digest:
                    raise ValueError(f'publisher size/SHA256 mismatch: {url}')
                stream.flush()
                os.fsync(stream.fileno())
            os.link(temporary, target)  # Never replace a concurrently created or historical source.
            temporary.unlink()
            return (str(target), digest, *identity(target))
        except (OSError, EOFError) as error:
            if attempt == 2:
                raise
            log(f'retry {name}: {error}')
            time.sleep(2 ** attempt)
        finally:
            if temporary is not None and temporary.exists():
                temporary.unlink()
    raise RuntimeError('unreachable download retry state')


def resolved_assets(database, assets):
    resolved = []
    for asset in sorted(assets):
        path = verified_asset(database, asset)
        if path is None:
            raise ValueError(f'missing publisher-verified selected asset: {asset[0]} {asset[1]}')
        resolved.append((asset, path, identity(Path(path))))
    return resolved


def input_signature(selected):
    return repr(selected)


def archive_checks(database):
    if not database.execute("SELECT 1 FROM sqlite_master WHERE name='archive_checks'").fetchone():
        return {}
    return {tag: (inputs, error) for tag, inputs, error in database.execute('SELECT * FROM archive_checks')}


def alternative_exports(day, preferred, releases):
    tags = {release['tag_name']: release for release, _ in releases.values()}
    for tag, release in sorted(tags.items()):
        if tag.startswith('v' + day.replace('-', '.') + '-planes-readsb-') and tag != preferred and 'mlatonly' not in tag:
            yield release_assets(day, [a['browser_download_url'] for a in release['assets']], releases)


def effective_assets(database, preferred, releases, requested):
    """Published preference changes only after identity-bound proof of broken TAR continuity."""
    checks = archive_checks(database)
    selected = []
    for day in sorted(requested):
        assets = sorted(release_assets(day, preferred[day], releases))
        tag = assets[0][5]
        if tag in checks:
            inputs, error = checks[tag]
            if inputs != input_signature(resolved_assets(database, assets)):
                raise ValueError(f'{day}: changed archive inspection inputs: {tag}')
            if error:
                candidates = []
                for alternative in alternative_exports(day, tag, releases):
                    checked = checks.get(alternative[0][5])
                    if checked is None:
                        continue
                    if checked[0] != input_signature(resolved_assets(database, alternative)):
                        raise ValueError(f'{day}: changed alternative inspection inputs')
                    if not checked[1]:
                        candidates.append(alternative)
                if len(candidates) != 1:
                    raise ValueError(f'{day}: require one verified complete alternative, found {len(candidates)}')
                assets = candidates[0]
                print(f'GA source recovery: day={day} rejected={tag} reason={error} selected={assets[0][5]}', file=sys.stderr)
        selected.extend(assets)
    return sorted(selected)


class StrictTarInfo(tarfile.TarInfo):
    @classmethod
    def fromtarfile(cls, archive):
        try:
            return super().fromtarfile(archive)
        except (tarfile.InvalidHeaderError, tarfile.TruncatedHeaderError, tarfile.SubsequentHeaderError) as error:
            # ignore_zeros accepts concatenated TAR streams, but must never skip bad headers.
            raise tarfile.ReadError(f'offset {archive.offset}: {error}') from error


class ArchiveParts:
    def __init__(self, files):
        self.files, self.offsets, self.position = files, [0], 0
        for source in files:
            self.offsets.append(self.offsets[-1] + os.fstat(source.fileno()).st_size)

    def tell(self):
        return self.position

    def seek(self, offset, whence=0):
        self.position = offset + (self.position if whence == 1 else self.offsets[-1] if whence == 2 else 0)
        return self.position

    def read(self, size):
        # Only extension headers are read in bulk; ordinary payloads are seek-skipped.
        # Refuse unreasonable metadata without labelling it corrupt or selecting a fallback.
        if size > 1024 * 1024:
            raise ValueError('TAR inspection metadata exceeds 1 MiB')
        chunks = []
        while size and self.position < self.offsets[-1]:
            index = bisect.bisect_right(self.offsets, self.position) - 1
            source = self.files[index]
            source.seek(self.position - self.offsets[index])
            chunk = source.read(min(size, self.offsets[index + 1] - self.position))
            if not chunk:
                raise OSError('source changed during TAR inspection')
            chunks.append(chunk)
            self.position += len(chunk)
            size -= len(chunk)
        return b''.join(chunks)


def check_archive_continuity(selected):
    """Bounded GNU/PAX-aware structural walk; native Stage0 still owns gzip/JSON validation."""
    split = [path for asset, path, _ in selected if not asset[1].endswith('.tar')]
    streams = ([split] if split else []) + [[path] for asset, path, _ in selected if asset[1].endswith('.tar')]
    for paths in streams:
        with ExitStack() as stack:
            source = ArchiveParts([stack.enter_context(Path(path).open('rb')) for path in paths])
            total = source.offsets[-1]
            if total < 1024 or total % 512:
                raise tarfile.ReadError('incomplete TAR byte length')
            source.seek(-1024, 2)
            if source.read(1024) != bytes(1024):
                raise tarfile.ReadError('missing TAR end marker')
            source.seek(0)
            with tarfile.open(fileobj=source, mode='r:', ignore_zeros=True, tarinfo=StrictTarInfo) as archive:
                for _ in archive:
                    archive.members.clear()
    if any(identity(Path(path)) != before for _, path, before in selected):
        raise OSError('source changed during TAR inspection')


def preserved_catalog(database):
    days = {date.fromisoformat(row[0]) for row in database.execute('SELECT day FROM window')}
    def preserved_response(url):
        row = database.execute('SELECT body FROM responses WHERE url=?', (url,)).fetchone()
        if row is None:
            raise ValueError(f'missing preserved publisher response: {url}')
        return row[0]
    preferred, releases = preferred_releases(database, days, preserved_response)
    assets = sorted(asset for day, urls in preferred.items() for asset in release_assets(day, urls, releases))
    if assets != database.execute('SELECT day,name,url,size,sha256,tag FROM assets ORDER BY day,name').fetchall():
        raise ValueError('catalog asset rows differ from preserved official responses')
    return preferred, releases


def recover_sources(root, requested, reserve_bytes):
    """Acquire verified alternatives for proven broken TARs, then atomically admit their inspections."""
    if reserve_bytes < 0:
        raise ValueError('recovery requires a nonnegative free-space reserve')
    with closing(sqlite3.connect(f'{(root / "catalog.sqlite").resolve().as_uri()}?mode=rw', uri=True)) as database:
        preferred, releases = preserved_catalog(database)
        if not requested or not requested <= preferred.keys():
            raise ValueError('recovery dates are empty or outside the selected source catalog')
        inspected = []
        for day in sorted(requested):
            assets = release_assets(day, preferred[day], releases)
            tag = assets[0][5]
            if 'mlatonly' in tag:
                raise ValueError(f'{day}: MLAT-only coverage cannot be recovered as a complete day')
            selected = resolved_assets(database, assets)
            try:
                check_archive_continuity(selected)
            except tarfile.ReadError as failure:
                inspected.append((tag, input_signature(selected), str(failure), selected))
            else:
                raise ValueError(f'{day}: preferred TAR is structurally readable; no recovery admitted')
            alternatives = list(alternative_exports(day, tag, releases))
            # A separate native parent contains exactly this export's selected TAR streams.
            pending = [asset for candidate in alternatives for asset in candidate
                       if verified_asset(database, asset) is None]
            needed = sum(asset[3] for asset in pending
                         if not asset_target_path(asset, root / 'recovery' / asset[5]).exists())
            if shutil.disk_usage(root).free < needed + reserve_bytes:
                raise ValueError('insufficient disk space for recovery bytes plus reserve')
            for asset in pending:
                result = acquire(asset, root / 'recovery' / asset[5], reserve_bytes)
                with database:
                    database.execute('INSERT OR REPLACE INTO verified VALUES (?,?,?,?,?,?,?)', result)
                log(f'ACQUIRED recovery bytes={result[4]} sha256={result[1]} path={result[0]}')
            valid = 0
            for candidate in alternatives:
                selected = resolved_assets(database, candidate)
                try:
                    check_archive_continuity(selected)
                    error = ''
                except tarfile.ReadError as failure:
                    error = str(failure)
                valid += int(not error)
                inspected.append((candidate[0][5], input_signature(selected), error, selected))
            if valid != 1:
                raise ValueError(f'{day}: require one verified complete alternative, found {valid}')
        with database:
            database.execute('BEGIN IMMEDIATE')
            if preserved_catalog(database) != (preferred, releases):
                raise ValueError('publisher catalog changed during recovery')
            for _, signature, _, selected in inspected:
                if input_signature(resolved_assets(database, [asset for asset, _, _ in selected])) != signature:
                    raise ValueError('archive inputs changed during recovery')
            database.execute('CREATE TABLE IF NOT EXISTS archive_checks (tag TEXT PRIMARY KEY, inputs TEXT NOT NULL, error TEXT NOT NULL)')
            database.executemany('INSERT OR REPLACE INTO archive_checks VALUES (?,?,?)', [row[:3] for row in inspected])
            return effective_assets(database, preferred, releases, requested)


def validate_selected_sources(root, requested):
    """Resolve only requested days against preserved publisher authority; never fetch or write."""
    database = sqlite3.connect(f'{(root / "catalog.sqlite").resolve().as_uri()}?mode=ro', uri=True)
    database.execute('PRAGMA query_only=ON')
    try:
        days = tuple(date.fromisoformat(row[0]) for row in database.execute('SELECT day FROM window ORDER BY day'))
        if not days or not requested or not requested <= {day.isoformat() for day in days}:
            raise ValueError('requested days are empty or outside the selected source catalog')
        preferred, releases = preserved_catalog(database)
        selected = effective_assets(database, preferred, releases, requested)
        insufficient = sorted({a[0] for a in selected if 'mlatonly' in a[5]})
        selected = [asset for asset in selected if 'mlatonly' not in asset[5]]
        if not selected:
            raise ValueError(f'no complete source days; omitted MLAT-only dates: {insufficient}')
        resolved = resolved_assets(database, selected)
        if insufficient:
            print(f'GA source coverage: calendar_dates={len(requested)} sampled_days={len({asset[0] for asset in selected})} omitted_mlatonly_days={insufficient}', file=sys.stderr)
        return resolved
    finally:
        database.close()


def source_receipt(work, selected, stage, class_filter, action):
    """A successful native write anchors output stats to the selected publisher assets."""
    path = work / 'source-receipts.sqlite'
    record = action == 'complete'
    if action != 'check':
        database = sqlite3.connect(path)
        database.execute('CREATE TABLE IF NOT EXISTS sources (day, name, url, size, sha256, tag, source_id, class_filter, PRIMARY KEY(day,name))')
        database.execute('CREATE TABLE IF NOT EXISTS pending (day, stage, inputs, PRIMARY KEY(day,stage))')
        database.execute('CREATE TABLE IF NOT EXISTS artifacts (day, stage, path, dev, ino, size, mtime_ns, ctime_ns, PRIMARY KEY(day,stage))')
    else:
        database = sqlite3.connect(f'{path.resolve().as_uri()}?mode=ro', uri=True)
    grouped = {}
    for asset, _, _ in selected:
        grouped.setdefault(asset[0], []).append((*asset, 0, class_filter))
    try:
        with database:
            for day, expected in grouped.items():
                stored = database.execute('SELECT * FROM sources WHERE day=? ORDER BY name', (day,)).fetchall()
                if action == 'check' or stage == 'segments':
                    if stored != expected:
                        raise ValueError(f'{day}: selected source/feed/class differs from completed Stage0 receipt')
                stages = ['flights', 'segments'] if stage == 'segments' and action == 'check' else ['flights']
                if action == 'check' or stage == 'segments':
                    for prerequisite in stages:
                        artifact = work / prerequisite / f'{day}.arrow'
                        actual = (day, prerequisite, str(artifact.resolve()), *identity(artifact))
                        receipt = database.execute('SELECT * FROM artifacts WHERE day=? AND stage=?', (day, prerequisite)).fetchone()
                        if receipt != actual:
                            raise ValueError(f'{day}: missing or changed completed {prerequisite} receipt')
                inputs = input_signature([(asset, source, before) for asset, source, before in selected if asset[0] == day])
                if stage == 'segments':
                    inputs += repr(identity(work / 'flights' / f'{day}.arrow'))
                if action == 'begin':
                    database.execute('INSERT OR REPLACE INTO pending VALUES (?,?,?)', (day, stage, inputs))
                if record:
                    pending = database.execute('SELECT inputs FROM pending WHERE day=? AND stage=?', (day, stage)).fetchone()
                    if pending != (inputs,):
                        raise ValueError(f'{day}: source or parent changed since {stage} began')
                    artifact = work / stage / f'{day}.arrow'
                    actual = (day, stage, str(artifact.resolve()), *identity(artifact))
                    if stage == 'flights':
                        database.execute('DELETE FROM sources WHERE day=?', (day,))
                        database.execute('DELETE FROM artifacts WHERE day=?', (day,))
                        database.executemany('INSERT INTO sources VALUES (?,?,?,?,?,?,?,?)', expected)
                    database.execute('INSERT OR REPLACE INTO artifacts VALUES (?,?,?,?,?,?,?,?)', actual)
                    database.execute('DELETE FROM pending WHERE day=? AND stage=?', (day, stage))
            for _, source, before in selected:
                if identity(Path(source)) != before:
                    raise ValueError(f'selected source changed during validation: {source}')
    finally:
        database.close()


def validate_main(arguments):
    parser = argparse.ArgumentParser(description='Validate selected GA source identity without acquisition')
    parser.add_argument('--source-root', type=Path, required=True)
    parser.add_argument('--days', help='Requested dates; absent means the complete catalog calendar')
    parser.add_argument('--work-dir', type=Path)
    parser.add_argument('--stage', choices=['flights', 'segments'])
    parser.add_argument('--class-filter', choices=['all', 'ga', 'non-ga'], default='ga')
    parser.add_argument('--action', choices=['check', 'begin', 'complete'], default='check')
    args = parser.parse_args(arguments)
    if bool(args.work_dir) != bool(args.stage) or (args.action != 'check' and not args.stage):
        parser.error('work directory and stage are required together for a completion receipt')
    try:
        if args.days is None:
            with sqlite3.connect(f'{(args.source_root / "catalog.sqlite").resolve().as_uri()}?mode=ro', uri=True) as database:
                requested = {row[0] for row in database.execute('SELECT day FROM window')}
        else:
            raw_days = args.days.split(',')
            requested = {date.fromisoformat(day).isoformat() for day in raw_days}
            if len(requested) != len(raw_days):
                raise ValueError('duplicate requested source days')
        selected = validate_selected_sources(args.source_root, requested)
        if args.stage:
            if {asset[0] for asset, _, _ in selected} != requested:
                raise ValueError('work receipts require only selected complete-source days')
            source_receipt(args.work_dir, selected, args.stage, args.class_filter, args.action)
        # NUL-separated paths are transport for the native archive selector,
        # not another source manifest or a replacement for the SQLite authority.
        for asset, path, _ in selected:
            sys.stdout.buffer.write(asset[0].encode() + b'\0' + os.fsencode(path) + b'\0')
    except (OSError, ValueError, sqlite3.Error, KeyError) as error:
        parser.exit(1, f'selected source/cache validation failed: {error}\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--anchor', help='YYYY-MM; shared completed monthly sample anchor')
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--verified-local', type=Path, help='Independent checksum and unchanged-stat SQLite receipt')
    parser.add_argument('--catalog-only', action='store_true')
    parser.add_argument('--days', help='Explicit acquisition subset within the complete requested GA window')
    parser.add_argument('--workers', type=int, default=4)
    parser.add_argument('--reserve-bytes', type=int, required=True)
    args = parser.parse_args()
    try:
        days = sampling_days(resolve_anchor(args.anchor, datetime.now(timezone.utc).date()))[1]
        selected = set(args.days.split(',')) if args.days else {day.isoformat() for day in days}
        if not selected or not selected <= {day.isoformat() for day in days} or not 1 <= args.workers <= 8 or args.reserve_bytes < 0:
            raise ValueError('require in-window acquisition dates, 1..8 workers and nonnegative reserve')
        with open_catalog(args.out, days) as database:
            import_verified(database, args.verified_local)
            preferred, releases = preserved_catalog(database)
            assets = effective_assets(database, preferred, releases, preferred.keys())
            unavailable = sorted({a[0] for a in assets if 'mlatonly' in a[5]})
            pending = [a for a in assets if a[0] in selected and 'mlatonly' not in a[5] and not verified_asset(database, a)]
            needed = sum(a[3] for a in pending if not asset_target_path(a, args.out).exists())
            log(f'window={len(days)} selected={len(selected)} pending_assets={len(pending)} pending_bytes={needed} insufficient_mlatonly_days={unavailable}')
            if args.catalog_only:
                return
            if shutil.disk_usage(args.out).free < needed + args.reserve_bytes:
                raise ValueError('insufficient disk space for pending bytes plus reserve')
            with ThreadPoolExecutor(max_workers=args.workers) as pool:
                remaining = iter(pending)
                futures = {pool.submit(acquire, asset, args.out, args.reserve_bytes) for asset in islice(remaining, args.workers)}
                while futures:
                    done, futures = wait(futures, return_when=FIRST_COMPLETED)
                    for future in done:
                        result = future.result()
                        with database:
                            database.execute('INSERT OR REPLACE INTO verified VALUES (?,?,?,?,?,?,?)', result)
                        log(f'ACQUIRED bytes={result[4]} sha256={result[1]} path={result[0]}')
                    futures.update(pool.submit(acquire, asset, args.out, args.reserve_bytes) for asset in islice(remaining, len(done)))
            for asset in assets:
                if asset[0] in selected and 'mlatonly' not in asset[5] and not verified_asset(database, asset):
                    raise ValueError(f'asset remains unavailable: {asset[0]} {asset[1]}')
            log(f'ACQUISITION_FINISHED selected={len(selected)} insufficient_source_days={unavailable}; not a Stage0 or popup acceptance claim')
    except (ValueError, OSError, KeyError) as error:
        parser.exit(1, f'{error}\n')


if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == 'recover':
        parser = argparse.ArgumentParser(description='Acquire and admit publisher-verified complete alternatives for structurally corrupt preferred exports; retain all inspected originals unchanged')
        parser.add_argument('--source-root', type=Path, required=True)
        parser.add_argument('--days', required=True)
        parser.add_argument('--reserve-bytes', type=int, required=True)
        args = parser.parse_args(sys.argv[2:])
        try:
            requested = {date.fromisoformat(day).isoformat() for day in args.days.split(',')}
            recover_sources(args.source_root, requested, args.reserve_bytes)
        except (ValueError, OSError, KeyError, sqlite3.Error, tarfile.TarError) as error:
            parser.exit(1, f'source recovery failed: {error}\n')
    elif len(sys.argv) > 1 and sys.argv[1] == 'validate':
        validate_main(sys.argv[2:])
    else:
        main()
