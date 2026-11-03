#!/usr/bin/env python3
"""Pin the completed climatology's counts and absorption means as one slim statistics object."""
import argparse
import hashlib
import json
import shutil
from pathlib import Path
import zipfile


def digest(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--state', type=Path, required=True)
    parser.add_argument('--receipt', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    receipt = json.loads(args.receipt.read_text())
    if receipt['file'] != args.state.name or receipt['sha256'] != digest(args.state):
        raise ValueError('The completed state differs from its receipt')
    members = {'counts': 'counts.npy', 'favourable': 'favourable_counts.npy', 'means': 'means.npy', 'receipt': 'receipt.json'}
    args.out.parent.mkdir(parents=True, exist_ok=True)
    temporary = args.out.with_name(args.out.name + '.tmp')
    with zipfile.ZipFile(args.state) as source, zipfile.ZipFile(temporary, 'w') as output:
        for name in ('counts', 'favourable', 'means'):
            with source.open(members[name]) as reader, output.open(members[name], 'w', force_zip64=True) as writer:
                shutil.copyfileobj(reader, writer, 1 << 20)
        output.writestr(members['receipt'], json.dumps(receipt, sort_keys=True))
    if args.out.exists():
        if digest(temporary) != digest(args.out):
            raise ValueError('Refusing to replace a different statistics input')
        temporary.unlink()
    else:
        temporary.rename(args.out)
    print(json.dumps({'sha256': digest(args.out), 'sampling_hours': int(receipt['identity']['contract']['sampling_hours']), **members}, indent=2))


if __name__ == '__main__':
    main()
