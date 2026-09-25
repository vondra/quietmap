#!/usr/bin/env python3
"""Fetch AHN4 DTM 0.5 m as retained 5 m WCS windows over the official tile index."""
import argparse
import json
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import time
import urllib.request

import numpy as np
from osgeo import gdal

from dem_windows import LandMask, download_bytes, nodata_tag, reproject_bounds, split_wcs_multipart
from terrain_io import digest, fetch, provenance, publish_bytes, publish_json, publish_source_json, utc_now

gdal.UseExceptions()

PROVIDER = 'nl-ahn4'
INDEX_URL = 'https://service.pdok.nl/rws/ahn/atom/downloads/dtm_05m/kaartbladindex.json'
WCS = 'https://service.pdok.nl/rws/ahn/wcs/v1_0'
COVERAGE = 'dtm_05m'
LICENCE = 'CC0 1.0'
LICENCE_URL = 'https://creativecommons.org/publicdomain/zero/1.0/'
RESOLUTION = 5


def window_request(x0, y0, x1, y1):
    width, height = round((x1 - x0) / RESOLUTION), round((y1 - y0) / RESOLUTION)
    query = (f'SERVICE=WCS&VERSION=2.0.1&REQUEST=GetCoverage&COVERAGEID={COVERAGE}'
             f'&SUBSET=x({x0},{x1})&SUBSET=y({y0},{y1})'
             f'&SCALESIZE=x({width}),y({height})&FORMAT=image/tiff')
    return query, width, height


def fetch_window(output, feature, mask, position, total):
    name = feature['properties']['name'].replace('.tif', '')
    target = output / PROVIDER / f'ahn4_dtm_5m_{name}.tif'
    receipt = Path(str(target) + '.provenance.json')
    ring = feature['geometry']['coordinates'][0]
    x0, y0 = min(p[0] for p in ring), min(p[1] for p in ring)
    x1, y1 = max(p[0] for p in ring), max(p[1] for p in ring)
    query, width, height = window_request(x0, y0, x1, y1)
    url = WCS + '?' + query
    if target.exists() and receipt.exists():
        record = provenance(target)
        if record.get('request_url') != url:
            raise ValueError(f'retained window references another request: {target}')
        return record
    if mask is not None and not mask.has_land(*reproject_bounds(x0, y0, x1, y1, 28992)):
        print(json.dumps({'skipped_sea': name}), flush=True)
        return None
    started = time.monotonic()
    body, headers = download_bytes(url)
    payload = split_wcs_multipart(body, headers['Content-Type'])
    publish_bytes(target, payload)
    dataset = gdal.Open(str(target))
    transform = dataset.GetGeoTransform()
    if (dataset.RasterXSize, dataset.RasterYSize) != (width, height):
        raise ValueError(f'unexpected AHN window grid: {name}')
    if not (abs(transform[0] - x0) < 0.01 and abs(transform[3] - y1) < 0.01
            and abs(transform[1] - RESOLUTION) < 1e-9):
        raise ValueError(f'unexpected AHN window placement: {name}')
    values = dataset.ReadAsArray()
    nodata = dataset.GetRasterBand(1).GetNoDataValue()
    if bool(((values == nodata) | ~np.isfinite(values)).all()):
        target.unlink()
        print(json.dumps({'skipped_sea': name}), flush=True)
        return None
    record = dict(url=INDEX_URL, request_url=url, fetched_utc=utc_now(), sha256=digest(target),
                  bytes=target.stat().st_size, licence=LICENCE, licence_url=LICENCE_URL,
                  terms_checked_utc='2026-09-25', raw_bytes_retained=False,
                  notes=('AHN4 DTM 0.5 m (ALS 2020-2022) WCS window at 5 m over official tile '
                         f'{name}; RD New (EPSG:28992), NAP heights (EPSG:5709); raw 0.5 m bytes '
                         'not retained.'))
    publish_json(receipt, record)
    print(json.dumps(dict(done=position, total=total, path=str(target),
                           seconds=time.monotonic() - started)), flush=True)
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--land-mask', type=Path, default=None)
    parser.add_argument('--jobs', type=int, default=4)
    parser.add_argument('--max-items', type=int, default=0)
    args = parser.parse_args()
    if not 1 <= args.jobs <= 8:
        parser.error('jobs must stay within the batch thread budget')
    output = args.output
    fetch(output, PROVIDER, 'kaartbladindex.json', INDEX_URL, LICENCE, LICENCE_URL,
          '2026-09-25', 'Official AHN4 DTM 0.5 m tile index (kaartblad); 1373 tiles.')
    features = json.loads((output / PROVIDER / 'kaartbladindex.json').read_text())['features']
    if args.max_items:
        features = features[:args.max_items]
    mask = LandMask(args.land_mask) if args.land_mask else None
    print(json.dumps({'tiles': len(features)}), flush=True)

    def worker(entry):
        position, feature = entry
        started = time.monotonic()
        try:
            return fetch_window(output, feature, mask, position, len(features))
        finally:
            time.sleep(max(0, 1 - (time.monotonic() - started)))

    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        records = list(pool.map(worker, enumerate(features, 1)))
    kept = [(feature, record) for feature, record in zip(features, records) if record]
    tags = {nodata_tag(output / PROVIDER /
                       f'ahn4_dtm_5m_{feature["properties"]["name"].replace(".tif", "")}.tif')
            for feature, _ in kept}
    if len(tags) != 1:
        raise ValueError(f'mixed nodata tags in AHN windows: {tags}')
    nodata = tags.pop()
    sources = [dict(path=str((output / PROVIDER /
                              f'ahn4_dtm_5m_{feature["properties"]["name"].replace(".tif", "")}.tif').resolve()),
                    horizontal_crs='EPSG:28992', vertical_crs=5709, epoch='AHN4 2020-2022',
                    role='national', group='NL-AHN4', nodata=nodata,
                    datum_area_of_interest=[3.0, 50.7, 7.3, 53.7]) for feature, _ in kept]
    publish_source_json(output, output / PROVIDER / 'country-sources.json', sources)
    publish_json(output / PROVIDER / 'skipped-sea.json',
                 sorted(feature['properties']['name'] for feature, record in zip(features, records)
                        if not record))
    print(json.dumps({'kept': len(kept), 'skipped': len(features) - len(kept)}), flush=True)


if __name__ == '__main__':
    main()
