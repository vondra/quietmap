#!/usr/bin/env python3
"""Fetch selected Bavarian DGM5 archive names from the official Metalink catalogue."""
import argparse
from pathlib import Path
import xml.etree.ElementTree as ET
from terrain_io import fetch

CATALOGUE = 'https://geodaten.bayern.de/odd/a/dgm/dgm5xyz/meta/metalink/09.meta4'
LICENCE_URL = 'https://creativecommons.org/licenses/by/4.0/'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--tile', action='append', default=[], help='Exact archive name from the catalogue; omit to list')
    args = parser.parse_args()
    provider = 'de-by-dgm5'
    fetch(args.output, provider, 'catalogue.meta4', CATALOGUE, 'CC BY 4.0', LICENCE_URL, '2026-09-24')
    files = ET.parse(args.output / provider / 'catalogue.meta4').findall('.//{*}file')
    wanted = set(args.tile)
    for entry in files:
        name = entry.attrib['name']
        if not wanted:
            print(name)
            continue
        if name in wanted:
            url = entry.find('{*}url').text
            sha = entry.find("{*}hash[@type='sha-256']")
            fetch(args.output, provider, name, url, 'CC BY 4.0', LICENCE_URL, '2026-09-24',
                  'Bayerische Vermessungsverwaltung DGM5 XYZ; ETRS89/UTM32, DHHN2016 (EPSG:7837).',
                  expected_sha256=sha.text if sha is not None else None)
    absent = wanted - {entry.attrib['name'] for entry in files}
    if absent:
        raise ValueError(f'not in official catalogue: {sorted(absent)}')


if __name__ == '__main__':
    main()
