#!/usr/bin/env python3
"""Fetch swissALTI3D 0.5 m tiles as retained 4 m overview rasters (stream-reduced)."""
import argparse
import json
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import time
import urllib.request

from cog_reduce import reduce_cog_overview
from terrain_io import digest, provenance, publish_json, publish_source_json, utc_now

PROVIDER = 'ch-alti3d'
COLLECTION = 'https://data.geo.admin.ch/api/stac/v1/collections/ch.swisstopo.swissalti3d/items?limit=100'
LICENCE = 'swisstopo OGD (free use with source indication)'
LICENCE_URL = 'https://www.swisstopo.admin.ch/en/free-geodata-ogd'
OVERVIEW_INDEX = 2
TILE_SIZES = (250, 250)


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
    assets = [(name, body) for name, body in item['assets'].items() if name.endswith('.tif')]
    if len(assets) != 1 or not assets[0][0].endswith('_0.5_2056_5728.tif'):
        raise ValueError(f'unexpected swissALTI3D asset: {item["id"]}')
    return assets[0]


def reduce_item(output, item, position, total):
    name, body = asset_of(item)
    target = output / PROVIDER / (item['id'] + '_4m.tif')
    receipt = Path(str(target) + '.provenance.json')
    if target.exists() and receipt.exists():
        record = provenance(target)
        if record.get('parent_href') != body['href']:
            raise ValueError(f'retained tile references another parent: {target}')
        return record
    started = time.monotonic()
    columns, rows, factor, nodata = reduce_cog_overview(body['href'], OVERVIEW_INDEX, target)
    if (columns, rows) != TILE_SIZES or factor != 8 or nodata != -9999:
        raise ValueError(f'unexpected swissALTI3D overview grid: {item["id"]}')
    record = dict(url=body['href'], fetched_utc=utc_now(), sha256=digest(target),
                  bytes=target.stat().st_size, licence=LICENCE, licence_url=LICENCE_URL,
                  terms_checked_utc='2026-09-25', parent_href=body['href'],
                  parent_sha512=body.get('file:checksum'), raw_bytes_retained=False,
                  notes=('swissALTI3D 0.5 m COG overview level 2 (4 m, exact 8x reduction) retained; '
                         'LV95 (EPSG:2056), LN02 heights (EPSG:5728); raw 0.5 m bytes not retained; '
                         'parent checksum from STAC, re-verifiable by re-reading the overview.'))
    publish_json(receipt, record)
    print(json.dumps(dict(done=position, total=total, path=str(target),
                           seconds=time.monotonic() - started)), flush=True)
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
    items = enumerate_items()
    if args.max_items:
        items = items[:args.max_items]
    print(json.dumps({'items': len(items)}), flush=True)

    def worker(entry):
        position, item = entry
        started = time.monotonic()
        try:
            return reduce_item(output, item, position, len(items))
        finally:
            time.sleep(max(0, 1 - (time.monotonic() - started)))

    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        list(pool.map(worker, enumerate(items, 1)))
    years = sorted({item['properties']['datetime'][:4] for item in items})
    epoch = f'{years[0]}-{years[-1]} swissALTI3D 6-year update cycle' if years else 'unknown'
    sources = [dict(path=str((output / PROVIDER / (item['id'] + '_4m.tif')).resolve()),
                    horizontal_crs='EPSG:2056', vertical_crs=5728, epoch=epoch,
                    role='national', group='CH-ALTI3D', nodata=-9999,
                    datum_area_of_interest=[5.9, 45.8, 10.6, 47.9]) for item in items]
    publish_source_json(output, output / PROVIDER / 'country-sources.json', sources)


if __name__ == '__main__':
    main()
