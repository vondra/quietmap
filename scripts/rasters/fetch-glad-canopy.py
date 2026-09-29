#!/usr/bin/env python3
"""Fetch catalogued GLAD 2020 forest-height tiles with provider licence and byte identities."""
import argparse
from pathlib import Path
import re
import time
from terrain_io import fetch

BASE = 'https://gladxfer.umd.edu/users/Potapov/GLCLUC2020/Forest_height_2020/'
LICENCE = 'https://glad.umd.edu/dataset/GLCLUC2020'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--tile', action='append', help='Exact catalogue basename; omit to list')
    parser.add_argument('--all', action='store_true')
    args = parser.parse_args()
    provider = 'glad-height-2020'
    fetch(args.output, provider, 'catalogue.html', BASE, 'CC BY', LICENCE, '2026-09-24')
    names = sorted(set(re.findall(r'href="(2020_\d+[NS]_\d+[EW]\.tif)"',
                                 (args.output / provider / 'catalogue.html').read_text())))
    if not names:
        raise ValueError('empty GLAD catalogue')
    if not args.tile and not args.all:
        print('\n'.join(names))
        return
    selected = names if args.all else args.tile
    if set(selected) - set(names):
        raise ValueError('requested tile is not in the official GLAD catalogue')
    for name in selected:
        started = time.monotonic()
        fetch(args.output, provider, name, BASE + name, 'CC BY', LICENCE, '2026-09-24',
              'Potapov et al. 2022, doi:10.3389/frsen.2022.856903; forest height 2020, metres; preserve nodata; raw bytes retained.')
        time.sleep(max(0, 1 - (time.monotonic() - started)))


if __name__ == '__main__':
    main()
