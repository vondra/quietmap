#!/usr/bin/env python3
"""Fetch DGT MDT 2 m 2024 tiles over an authenticated Centro de Dados session."""
import argparse
import html
import json
from concurrent.futures import ThreadPoolExecutor
import math
from pathlib import Path
import re
import threading
import time

import requests
from osgeo import gdal

from terrain_io import digest, provenance, publish_bytes, publish_json, publish_source_json, utc_now

gdal.UseExceptions()

PROVIDER = 'pt-dgt'
SEARCH = 'https://cdd.dgterritorio.gov.pt/dgt-be/v1/search'
LICENCE = 'CC BY 4.0'
LICENCE_URL = 'https://creativecommons.org/licenses/by/4.0/'
# Continental Portugal in PT-TM06; islands need their own datum review.
CONTINENTAL = (-120000, -300000, 200000, 300000)


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


def login(session, auth):
    page = session.get('https://cdd.dgterritorio.gov.pt/auth/login', timeout=60)
    page.raise_for_status()
    action = html.unescape(re.search(
        r'<form id="kc-form-login"[^>]*action="([^"]+)"', page.text).group(1))
    hidden = dict(re.findall(r'<input[^>]*name="([^"]+)"[^>]*value="([^"]*)"', page.text))
    response = session.post(action, data={**hidden, 'username': auth['username'],
                                         'password': auth['password'], 'credentialId': ''},
                            timeout=60)
    response.raise_for_status()
    if 'connect.sid' not in session.cookies:
        raise ValueError('CDD login did not establish a session')
    return response


def enumerate_items(year):
    body = {'collections': ['MDT-2m'], 'limit': 1000,
            'filter': {'op': 'like', 'args': [{'property': 'id'}, f'%-07-{year}']}}
    items = []
    session = requests.Session()
    session.headers['User-Agent'] = 'QuietMap terrain producer'
    while True:
        response = session.post(SEARCH, json=body, timeout=120)
        response.raise_for_status()
        page = response.json()
        features = page.get('features', [])
        items.extend(features)
        follow = next((link for link in page.get('links', []) if link.get('rel') == 'next'), None)
        if not follow or not features:
            break
        body = follow['body']
    return items


def fetch_tile(output, item, session, position, total):
    target = output / PROVIDER / (item['id'] + '.tif')
    receipt = Path(str(target) + '.provenance.json')
    href = item['assets']['data']['href']
    if target.exists() and receipt.exists():
        record = provenance(target)
        if record.get('item_id') != item['id']:
            raise ValueError(f'retained tile references another item: {target}')
        return item, record, None
    started = time.monotonic()
    try:
        response = session.get(href, timeout=300)
        response.raise_for_status()
        payload = response.content
        if len(payload) < 10000 or payload[:2] not in (b'II', b'MM'):
            return item, None, f'server returned {len(payload)} non-TIFF bytes'
    except Exception as error:
        return item, None, f'{type(error).__name__}: {str(error)[:200]}'
    publish_bytes(target, payload)
    dataset = gdal.Open(str(target))
    transform = dataset.GetGeoTransform()
    if transform[2] or transform[4]:
        raise ValueError(f'rotated MDT tile needs review: {item["id"]}')
    if abs(transform[1] - 2) > 0.05 or abs(transform[5] + 2) > 0.05:
        raise ValueError(f'unexpected MDT tile resolution: {item["id"]}')
    if dataset.GetRasterBand(1).GetNoDataValue() != -999:
        raise ValueError(f'unexpected MDT tile nodata: {item["id"]}')
    if not (CONTINENTAL[0] <= transform[0] <= CONTINENTAL[2]
            and CONTINENTAL[1] <= transform[3] - 1000 <= CONTINENTAL[3]):
        raise ValueError(f'MDT tile outside continental review: {item["id"]}')
    aligned_to_grid = (abs(transform[1] - 2) < 1e-9 and abs(transform[5] + 2) < 1e-9
                       and abs(transform[0] % 2) < 1e-9 and abs(transform[3] % 2) < 1e-9)
    if not aligned_to_grid:
        # Coastal edge tiles can sit half a cell off the national grid or carry
        # slightly non-square pixels; warp those rare tiles onto it (area average)
        # instead of dropping their land.
        south = transform[3] + dataset.RasterYSize * transform[5]
        east = transform[0] + dataset.RasterXSize * transform[1]
        bounds = (math.floor(transform[0] / 2) * 2, math.floor(south / 2) * 2,
                  math.ceil(east / 2) * 2, math.ceil(transform[3] / 2) * 2)
        dataset = None
        warped = gdal.Warp(str(target) + '.aligned', str(target), format='GTiff',
                           xRes=2, yRes=2, outputBounds=bounds, resampleAlg='average',
                           srcNodata=-999, dstNodata=-999, outputType=gdal.GDT_Float32,
                           creationOptions=['COMPRESS=DEFLATE', 'TILED=YES', 'PREDICTOR=2'])
        if warped is None:
            raise ValueError(f'MDT tile grid alignment failed: {item["id"]}')
        warped = None
        aligned = Path(str(target) + '.aligned')
        target.unlink()
        aligned.rename(target)
    record = dict(url=SEARCH, item_id=item['id'], fetched_utc=utc_now(),
                  sha256=digest(target), bytes=target.stat().st_size, licence=LICENCE,
                  licence_url=LICENCE_URL, terms_checked_utc='2026-09-25',
                  grid_aligned=aligned_to_grid,
                  notes=('DGT LiDAR MDT 2 m (Portugal continental, ALS 2024) tile; '
                         'PT-TM06 (EPSG:3763), Cascais heights (EPSG:5780); licence from SNIG '
                         'metadata record 077a8c94-8b46-4a8a-8796-0d7fc4662f0c; attribution '
                         '(c) Direcao-Geral do Territorio; download needs a CDD account; asset '
                         'hrefs are per-request and expire, the item id re-resolves them; '
                         'off-grid coastal tiles are area-averaged onto the 2 m national grid.'))
    publish_json(receipt, record)
    print(json.dumps(dict(done=position, total=total, path=str(target),
                           seconds=time.monotonic() - started)), flush=True)
    return item, record, None


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
    items = enumerate_items('2024')
    if args.max_items:
        items = items[:args.max_items]
    print(json.dumps({'items': len(items)}), flush=True)
    local = threading.local()

    def session_for_thread():
        session = getattr(local, 'session', None)
        if session is None:
            session = requests.Session()
            session.headers['User-Agent'] = 'QuietMap terrain producer'
            login(session, auth)
            local.session = session
        return session

    def worker(entry):
        position, item = entry
        started = time.monotonic()
        try:
            return fetch_tile(output, item, session_for_thread(), position, len(items))
        finally:
            time.sleep(max(0, 1 - (time.monotonic() - started)))

    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        results = list(pool.map(worker, enumerate(items, 1)))
    kept = [(item, record) for item, record, _ in results if record]
    failed = {item['id']: error for item, _, error in results if error}
    sources = [dict(path=str((output / PROVIDER / (item['id'] + '.tif')).resolve()),
                    horizontal_crs='EPSG:3763', vertical_crs=5780, epoch='ALS 2024',
                    role='national', group='PT-MDT2M', nodata=-999,
                    datum_area_of_interest=[-9.6, 36.8, -6.0, 42.2]) for item, _ in kept]
    publish_source_json(output, output / PROVIDER / 'country-sources.json', sources)
    publish_json(output / PROVIDER / 'failed-items.json', failed)
    print(json.dumps({'kept': len(kept), 'failed': len(failed)}), flush=True)


if __name__ == '__main__':
    main()
