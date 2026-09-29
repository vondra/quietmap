#!/usr/bin/env python3
"""Freeze the content of every world-build input: one SHA256SUMS per source family, every stored checksum verified."""

import argparse
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing
import hashlib
from itertools import batched
import json
import os
from pathlib import Path
import sqlite3
import sys
import tomllib

from world_build_inputs import input_files, source_family_roots, source_paths
from world_build_state import write_atomic

SUM_FILE_NAMES = ('SHA256SUMS', 'sha256.txt')
HASH_BATCH_FILES = 1024


def sha256_file(path):
    with path.open('rb') as source:
        digest = hashlib.file_digest(source, 'sha256').hexdigest()
        # Terabytes read once must not evict the served prepared data from the page cache.
        os.posix_fadvise(source.fileno(), 0, 0, os.POSIX_FADV_DONTNEED)
    return digest


def read_only_rows(database, query):
    # `immutable` opens without journal or lock files, so reading a receipt never writes beside it.
    with closing(sqlite3.connect(f'file:{database}?mode=ro&immutable=1', uri=True)) as connection:
        return connection.execute(query).fetchall()


def stored_digests(files):
    """Path -> {record: expected SHA-256} from the checksum records among a family's own files.

    Sum files list paths relative to themselves; `ships/download_gfw.py` receipts name `<name>.zip`;
    the `download-adsblol.py` catalog names publisher assets and verified local files, whose
    recorded directories may predate a disk migration, so both match by file name inside its tree.
    """
    expected = defaultdict(dict)
    by_name = defaultdict(list)
    for path in files:
        by_name[path.name].append(path)
    for record in files:
        if record.name in SUM_FILE_NAMES:
            for line in record.read_text().splitlines():
                if line.strip():
                    digest, name = line.split(None, 1)
                    expected[record.parent / name.strip().removeprefix('*')][str(record)] = digest.lower()
        elif record.name == 'receipts.sqlite':
            for name, digest in read_only_rows(record, 'SELECT name, sha256 FROM reports'):
                expected[record.parent / f'{name}.zip'][str(record)] = digest
        elif record.name == 'catalog.sqlite':
            for table, query in (('assets', 'SELECT name, sha256 FROM assets'),
                                 ('verified', 'SELECT path, sha256 FROM verified')):
                for name, digest in read_only_rows(record, query):
                    for path in by_name.get(Path(name).name, ()):
                        if record.parent in path.parents:
                            expected[path][f'{record}:{table}'] = digest
    return expected


def freeze_family(pool, family, source, roots, output):
    """Hash every file once; write `<family>.SHA256SUMS` if absent, else require identical content."""
    files = list(input_files(roots))
    base = source if source.is_dir() else source.parent
    expected = stored_digests(files)
    present = set(files)
    failures = [f'{path}: listed by {", ".join(records)} but missing'
                for path, records in expected.items() if path not in present and any(
                    Path(record).name in SUM_FILE_NAMES for record in records)]
    digests, total_bytes, checked = {}, 0, 0
    for batch in batched(files, HASH_BATCH_FILES):
        for path, digest in zip(batch, pool.map(sha256_file, batch)):
            digests[os.path.relpath(path, base)] = digest
            total_bytes += path.stat().st_size
            for record, want in expected.get(path, {}).items():
                checked += 1
                if want != digest:
                    failures.append(f'{path}: {record} expects {want}, the bytes hash to {digest}')
        print(f'{family}: hashed {len(digests)}/{len(files)} files', file=sys.stderr, flush=True)
    text = ''.join(f'{digest}  {name}\n' for name, digest in sorted(digests.items()))
    sums = output / f'{family}.SHA256SUMS'
    if sums.exists():
        frozen = {name: digest for digest, name in (line.split('  ', 1) for line in sums.read_text().splitlines())}
        changed = sorted(name for name in frozen.keys() | digests.keys() if frozen.get(name) != digests.get(name))
        failures += [f'{base / name}: differs from the frozen {sums}' for name in changed]
    elif not failures:
        write_atomic(sums, text)
    return failures, {'base': str(base), 'files': len(files), 'bytes': total_bytes,
                      'stored_checksums_checked': checked, 'sha256sums': str(sums),
                      'sha256sums_sha256': hashlib.sha256(text.encode()).hexdigest()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', type=Path, required=True, help='the world-build TOML whose [sources] are frozen')
    parser.add_argument('--output', type=Path, required=True, help='directory for <family>.SHA256SUMS, outside every source')
    parser.add_argument('--threads', type=int, default=2, help='parallel file reads (default 2)')
    args = parser.parse_args()
    if not 1 <= args.threads <= 8:
        parser.error('--threads must be 1..8')
    sources = source_paths(tomllib.loads(args.config.read_text()))
    output = args.output.resolve()
    for source in sources.values():
        if source == output or source in output.parents or output in source.parents:
            parser.error(f'output overlaps source {source}: the freeze never writes into inputs')
    output.mkdir(parents=True, exist_ok=True)
    failures, receipt = [], {}
    with ThreadPoolExecutor(args.threads) as pool:
        for family, roots in sorted(source_family_roots(sources).items()):
            family_failures, receipt[family] = freeze_family(pool, family, sources[family], roots, output)
            failures += family_failures
    print(json.dumps(receipt, indent=1, sort_keys=True))
    if failures:
        print('\n'.join(failures[:50]), file=sys.stderr)
        sys.exit(f'{len(failures)} checksum failures; no SHA256SUMS written for a failing family')


if __name__ == '__main__':
    main()
