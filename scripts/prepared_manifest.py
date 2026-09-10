#!/usr/bin/env python3
"""Pin prepared Arrow bytes in the SQLite manifest consumed by surface rendering."""

import argparse
import hashlib
import os
from pathlib import Path
import sqlite3
import tempfile
import sys

sys.path.insert(0, str(Path(__file__).parent / 'lib'))
import qmgrid


def file_identity(path):
    stat = path.stat()
    return stat.st_dev, stat.st_ino, stat.st_size, stat.st_mtime_ns, stat.st_ctime_ns


def sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as source:
        for chunk in iter(lambda: source.read(8 << 20), b''):
            digest.update(chunk)
    return digest.digest()


def square_directories(prepared):
    for x in sorted((prepared / 'z9').iterdir(), key=lambda path: path.name):
        if (not x.is_dir() or not x.name.isdecimal() or str(int(x.name)) != x.name
                or not 0 <= int(x.name) < qmgrid.Z9_AXIS):
            raise ValueError(f'not a canonical square column: {x}')
        for y in sorted(x.iterdir(), key=lambda path: path.name):
            if (not y.is_dir() or not all(name.isdecimal() and str(int(name)) == name
                    and 0 <= int(name) < qmgrid.Z9_AXIS for name in (x.name, y.name))):
                raise ValueError(f'not a canonical z9 square: {y}')
            yield y


def write_manifest(prepared, output):
    prepared, output = Path(prepared).resolve(), Path(output).absolute()
    output.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=f'.{output.name}.', dir=output.parent)
    os.close(descriptor)
    temporary = Path(temporary)
    files = total_bytes = 0
    try:
        with sqlite3.connect(temporary) as database:
            database.execute('BEGIN IMMEDIATE')
            database.execute('CREATE TABLE input_files('
                             'relative_path TEXT PRIMARY KEY, '
                             'sha256 BLOB NOT NULL CHECK(length(sha256)=32))')
            for square in square_directories(prepared):
                for path in sorted(square.glob('*.arrow')):
                    before = file_identity(path)
                    digest = sha256(path)
                    if before != file_identity(path):
                        raise ValueError(f'prepared input changed while hashing: {path}')
                    database.execute('INSERT INTO input_files VALUES(?,?)',
                                     (path.relative_to(prepared).as_posix(), digest))
                    files += 1
                    total_bytes += before[2]
            if not files:
                raise ValueError(f'no prepared Arrow inputs: {prepared}')
        with temporary.open('rb') as manifest:
            os.fsync(manifest.fileno())
        digest = sha256(temporary)
        os.replace(temporary, output)
        directory = os.open(output.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
        return dict(files=files, bytes=total_bytes, sha256=digest.hex())
    finally:
        temporary.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--prepared-dir', required=True)
    parser.add_argument('--output', required=True)
    args = parser.parse_args()
    import json
    print(json.dumps(write_manifest(args.prepared_dir, args.output)), flush=True)


if __name__ == '__main__':
    main()
