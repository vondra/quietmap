#!/usr/bin/env python3
"""Fetch Markhoejdmodell 1 m blocks as retained 8 m overview rasters (stream-reduced)."""
import argparse
import json
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import time
import urllib.request

from cog_reduce import reduce_cog_to_grid
from terrain_io import digest, provenance, publish_json, publish_source_json, utc_now

PROVIDER = 'se-lm'
COLLECTION = 'https://api.lantmateriet.se/stac-hojd/v1/collections/dtm-cog/items?limit=100'
LICENCE = 'CC-BY-4.0'
LICENCE_URL = 'https://creativecommons.org/licenses/by/4.0/'
RESOLUTION = 8


def read_credentials(path):
    values = {}
    for line in Path(path).read_text().splitlines():
        line = line.strip()
        if line and not line.startswith('#') and '=' in line:
            key, value = line.split('=', 1)
            values[key.strip()] = value.strip()
    if not values.get('username') or not values.get('password'):
        raise ValueError('credentials file lacks username/password')
    return values


def enumerate_items():
    items, url = [], COLLECTION
    while url:
        with urllib.request.urlopen(urllib.request.Request(
                url, headers={'User-Agent': 'QuietMap terrain producer'}), timeout=120) as response:
            page = json.load(response)
        items.extend(page['features'])
        url = next((link['href'] for link in page.get('links', []) if link.get('rel') == 'next'), None)
    return items


def reduce_item(output, item, auth, position, total):
    asset = item['assets']['data']
    target = output / PROVIDER / (item['id'] + '_8m.tif')
    receipt = Path(str(target) + '.provenance.json')
    if target.exists() and receipt.exists():
        record = provenance(target)
        if record.get('parent_href') != asset['href']:
            raise ValueError(f'retained block references another parent: {target}')
        return record
    started = time.monotonic()
    last = None
    for attempt in range(3):
        try:
            columns, rows, factor, nodata = reduce_cog_to_grid(
                asset['href'], RESOLUTION, target, auth['username'], auth['password'])
            break
        except RuntimeError as error:
            last = error
            time.sleep(2 ** attempt)
    else:
        raise last
    if nodata != -9999:
        raise ValueError(f'unexpected Markhoejdmodell nodata: {item["id"]}')
    record = dict(url=asset['href'], fetched_utc=utc_now(), sha256=digest(target),
                  bytes=target.stat().st_size, licence=LICENCE, licence_url=LICENCE_URL,
                  terms_checked_utc='2026-09-25', parent_href=asset['href'],
                  raw_bytes_retained=False,
                  notes=(f'Markhoejdmodell 1 m COG block, finest exact overview (level {factor}x) '
                         f'area-averaged to 8 m ({columns}x{rows}) on the absolute 8 m grid; '
                         'SWEREF 99 TM (EPSG:3006), RH2000 heights (EPSG:5613); raw 1 m bytes '
                         'not retained; download needs a Geotorget account holding the '
                         'Markhoejdmodell Nedladdning permission; attribution (c) Lantmateriet.'))
    publish_json(receipt, record)
    print(json.dumps(dict(done=position, total=total, path=str(target),
                           seconds=time.monotonic() - started)), flush=True)
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--credentials-file', type=Path, required=True)
    parser.add_argument('--jobs', type=int, default=4)
    parser.add_argument('--max-items', type=int, default=0)
    args = parser.parse_args()
    if not 1 <= args.jobs <= 8:
        parser.error('jobs must stay within the batch thread budget')
    output = args.output
    auth = read_credentials(args.credentials_file)
    items = enumerate_items()
    if args.max_items:
        items = items[:args.max_items]
    print(json.dumps({'items': len(items)}), flush=True)

    def worker(entry):
        position, item = entry
        started = time.monotonic()
        try:
            return reduce_item(output, item, auth, position, len(items))
        finally:
            time.sleep(max(0, 1 - (time.monotonic() - started)))

    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        list(pool.map(worker, enumerate(items, 1)))
    sources = [dict(path=str((output / PROVIDER / (item['id'] + '_8m.tif')).resolve()),
                    horizontal_crs='EPSG:3006', vertical_crs=5613,
                    epoch='Markhoejdmodell rolling ALS (2009-ongoing)', role='national',
                    group='SE-MHM', nodata=-9999,
                    datum_area_of_interest=[9.0, 55.1, 25.6, 69.1]) for item in items]
    publish_source_json(output, output / PROVIDER / 'country-sources.json', sources)


if __name__ == '__main__':
    main()
