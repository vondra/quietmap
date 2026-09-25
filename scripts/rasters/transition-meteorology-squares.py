#!/usr/bin/env python3
"""One-time transition: split the sha-verified global meteorology.arrow into per-square windows.

Runs once in the producer virtualenv (pyarrow reads the legacy global table), then is deleted:
the producer publishes squares directly from now on. Never re-fetches ERA5.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import shutil
import sys

import numpy as np
import pyarrow.ipc as ipc

from meteorology_io import all_squares, era5_window, sha256, verify_squares, write_squares

PERIODS = ('day', 'evening', 'night')


def flat_column(table, name, width, dtype):
    values = table[name].combine_chunks().values.to_numpy()
    if values.dtype != dtype or values.size % width:
        raise ValueError(f'Unexpected legacy column {name}')
    return values.reshape(-1, width)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--arrow', type=Path, required=True, help='Legacy global meteorology.arrow')
    parser.add_argument('--provenance', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True, help='Per-square rasters root')
    parser.add_argument('--workers', type=int, default=8, choices=range(1, 9))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    if shutil.disk_usage(args.output).free < 4_000_000_000:
        raise RuntimeError('Need 4 GB free for per-square publication')
    provenance = json.loads(args.provenance.read_text())
    if sha256(args.arrow) != provenance['sha256'] or args.arrow.stat().st_size != provenance['bytes']:
        raise ValueError('Legacy global table does not match its provenance')
    table = ipc.open_file(args.arrow).read_all()
    cells = 721 * 1440
    if table.num_rows != cells:
        raise ValueError('Legacy table has the wrong row count')
    if not np.array_equal(table['x'].combine_chunks().to_numpy(),
                          np.tile(np.arange(1440, dtype=np.uint16), 721)):
        raise ValueError('Legacy rows are not row-major')
    if not np.array_equal(table['y'].combine_chunks().to_numpy(),
                          np.repeat(np.arange(721, dtype=np.uint16), 1440)):
        raise ValueError('Legacy rows are not row-major')
    probabilities = np.stack([flat_column(table, f'p_{p}', 16, np.uint8) for p in PERIODS], axis=1)
    means = np.stack([flat_column(table, f'alpha_mean_{p}', 8, np.float32) for p in PERIODS], axis=1)
    variances = np.stack([flat_column(table, f'alpha_variance_{p}', 8, np.float32)
                          for p in PERIODS], axis=1)
    for k, period in enumerate(PERIODS):
        stored = table[f'p_max_{period}'].combine_chunks().to_numpy()
        if not np.array_equal(stored, probabilities[:, k].max(axis=1)):
            raise ValueError(f'Legacy p_max_{period} disagrees with its sectors')
    del table
    with ThreadPoolExecutor(max_workers=args.workers) as pool:
        stripes = list(pool.map(
            lambda x: write_squares(args.output, probabilities, means, variances,
                                    [(x, y) for y in range(512)]),
            range(512)))
    written = (sum(s[0] for s in stripes), sum(s[1] for s in stripes))
    if written[0] != 512 * 512:
        raise ValueError(f'Wrote {written[0]} squares, not 262144')
    files, total, digest = verify_squares(args.output)
    expected = 262144 * 16 + sum(era5_window(x, y)[2] * era5_window(x, y)[3] for x, y in all_squares()) * 240
    if (files, total) != (262144, expected):
        raise ValueError(f'Verification counted {(files, total)}, expected {(262144, expected)}')
    print(json.dumps(dict(arrow_sha256=provenance['sha256'], files=files, bytes=total,
                         squares_sha256=digest), indent=2))


if __name__ == '__main__':
    main()
