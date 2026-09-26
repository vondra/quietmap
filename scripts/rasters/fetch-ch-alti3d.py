#!/usr/bin/env python3
"""Fetch swissALTI3D 2 m tiles over the official STAC catalogue."""
import argparse
import hashlib
import json
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import re
import time
import urllib.request

from osgeo import gdal

from dem_windows import download_bytes, polite_sleep
from terrain_io import digest, provenance, publish_bytes, publish_json, publish_source_json, utc_now

gdal.UseExceptions()

PROVIDER = 'ch-alti3d'
COLLECTION = 'https://data.geo.admin.ch/api/stac/v1/collections/ch.swisstopo.swissalti3d/items?limit=100'
LICENCE = 'swisstopo OGD (free use with source indication)'
LICENCE_URL = 'https://www.swisstopo.admin.ch/en/free-geodata-ogd'
SUFFIX = '_2_2056_5728.tif'


def enumerate_items():
    items, url = [], COLLECTION
    while url:
        with urllib.request.urlopen(urllib.request.Request(
                url, headers={'User-Agent': 'QuietMap terrain producer'}), timeout=120) as response:
            page = json.load(response)
        items.extend(page['features'])
        url = next((link['href'] for link in page.get('links', []) if link.get('rel') == 'next'), None)
    return items


def asset_of(item):
    matches = [(name, body) for name, body in item['assets'].items() if name.endswith(SUFFIX)]
    if len(matches) != 1:
        raise ValueError(f'unexpected swissALTI3D 2 m asset: {item["id"]}')
    return matches[0]


def latest_per_tile(items):
    """Keep the newest item per 1 km tile; the catalogue holds every survey year."""
    newest = {}
    for item in items:
        key = re.search(r'(\d+-\d+)$', item['id'])
        if key is None:
            raise ValueError(f'unexpected swissALTI3D tile id: {item["id"]}')
        tile = key.group(1)
        if (tile not in newest
                or item['properties']['datetime'] > newest[tile]['properties']['datetime']):
            newest[tile] = item
    return sorted(newest.values(), key=lambda item: item['id'])


def fetch_item(output, item, position, total):
    name, body = asset_of(item)
    target = output / PROVIDER / name
    receipt = Path(str(target) + '.provenance.json')
    if target.exists() and receipt.exists():
        record = provenance(target)
        if record['url'] != body['href']:
            raise ValueError(f'retained tile references another parent: {target}')
        return record
    started = time.monotonic()
    checksum = (body.get('file:checksum') or '')
    payload, _ = download_bytes(body['href'])
    if len(checksum) == 64 and hashlib.sha256(payload).hexdigest() != checksum:
        raise ValueError(f'swissALTI3D checksum differs from STAC catalogue: {item["id"]}')
    publish_bytes(target, payload)
    record = dict(url=body['href'], fetched_utc=utc_now(), sha256=digest(target),
                  bytes=target.stat().st_size, licence=LICENCE, licence_url=LICENCE_URL,
                  terms_checked_utc='2026-09-25', raw_bytes_retained=True,
                  notes=('swissALTI3D 2 m tile; LV95 (EPSG:2056), LN02 heights (EPSG:5728); '
                         f'STAC parent checksum {checksum}; raw bytes retained.'))
    publish_json(receipt, record)
    dataset = gdal.Open(str(target))
    transform = dataset.GetGeoTransform()
    if (dataset.RasterXSize, dataset.RasterYSize) != (500, 500):
        raise ValueError(f'unexpected swissALTI3D grid: {item["id"]}')
    if abs(transform[1] - 2) > 1e-12 or dataset.GetRasterBand(1).GetNoDataValue() != -9999:
        raise ValueError(f'unexpected swissALTI3D placement: {item["id"]}')
    print(json.dumps(dict(done=position, total=total, path=str(target))), flush=True)
    polite_sleep(started)
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--jobs', type=int, default=4)
    parser.add_argument('--max-items', type=int, default=0)
    args = parser.parse_args()
    if not 1 <= args.jobs <= 8:
        parser.error('jobs must stay within the batch thread budget')
    output = args.output
    items = latest_per_tile(enumerate_items())
    if args.max_items:
        items = items[:args.max_items]
    print(json.dumps({'items': len(items)}), flush=True)

    def worker(entry):
        position, item = entry
        return fetch_item(output, item, position, len(items))

    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        list(pool.map(worker, enumerate(items, 1)))
    years = sorted({item['properties']['datetime'][:4] for item in items})
    epoch = f'{years[0]}-{years[-1]} swissALTI3D 6-year update cycle' if years else 'unknown'
    sources = [dict(path=str((output / PROVIDER / asset_of(item)[0]).resolve()),
                    horizontal_crs='EPSG:2056', vertical_crs=5728, epoch=epoch,
                    role='national', group='CH-ALTI3D', nodata=-9999,
                    datum_area_of_interest=[5.9, 45.8, 10.6, 47.9]) for item in items]
    publish_source_json(output, output / PROVIDER / 'country-sources.json', sources)


if __name__ == '__main__':
    main()
