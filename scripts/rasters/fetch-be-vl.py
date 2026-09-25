#!/usr/bin/env python3
"""Fetch Digitaal Hoogtemodel Vlaanderen II as retained 5 m WCS windows."""
import argparse
import json
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import time
import urllib.request

import numpy as np
from osgeo import gdal

from dem_windows import LandMask, grid_windows, reproject_bounds, split_wcs_multipart
from terrain_io import digest, fetch, provenance, publish_bytes, publish_json, publish_source_json, utc_now

gdal.UseExceptions()

PROVIDER = 'be-vl-dhmv'
WCS = 'https://geo.api.vlaanderen.be/DHMV/wcs'
COVERAGE = 'DHMVII_DTM_1m'
LICENCE = 'Modellicentie gratis hergebruik v1.0'
LICENCE_URL = 'https://data.vlaanderen.be/doc/licentie/modellicentie-gratis-hergebruik/v1.0'
METADATA = 'https://metadata.vlaanderen.be/srv/api/records/f52b1a13-86bc-4b64-8256-88cc0d1a8735/formatters/xml'
CRS = 31370
RESOLUTION = 5
STEP = 10000
EXTENT = (10000, 148000, 265000, 247000)


def window_request(x0, y0, x1, y1):
    width, height = round((x1 - x0) / RESOLUTION), round((y1 - y0) / RESOLUTION)
    crs = f'http://www.opengis.net/def/crs/EPSG/0/{CRS}'
    query = ('service=WCS&version=2.0.1&request=GetCoverage&CRS=EPSG:31370'
             f'&subset=x,{crs}({x0},{x1})&subset=y,{crs}({y0},{y1})'
             f'&SCALESIZE=x({width}),y({height})&COVERAGEID={COVERAGE}'
             '&FORMAT=image/geotiff&RESPONSE_CRS=EPSG:31370')
    return query, width, height


def fetch_window(output, window, mask, position, total):
    x0, y0, x1, y1 = window
    name = f'dhmv2_dtm_5m_{x0}_{y0}'
    target = output / PROVIDER / (name + '.tif')
    receipt = Path(str(target) + '.provenance.json')
    query, width, height = window_request(x0, y0, x1, y1)
    url = WCS + '?' + query
    if target.exists() and receipt.exists():
        record = provenance(target)
        if record.get('request_url') != url:
            raise ValueError(f'retained window references another request: {target}')
        return record
    if mask is not None and not mask.has_land(*reproject_bounds(x0, y0, x1, y1, CRS)):
        return None
    started = time.monotonic()
    request = urllib.request.Request(url, headers={'User-Agent': 'QuietMap terrain producer'})
    with urllib.request.urlopen(request, timeout=300) as response:
        payload = split_wcs_multipart(response.read(), response.headers['Content-Type'])
    publish_bytes(target, payload)
    dataset = gdal.Open(str(target))
    transform = dataset.GetGeoTransform()
    if (dataset.RasterXSize, dataset.RasterYSize) != (width, height):
        raise ValueError(f'unexpected DHMV window grid: {name}')
    if not (abs(transform[0] - x0) < 0.01 and abs(transform[3] - y1) < 0.01
            and abs(transform[1] - RESOLUTION) < 1e-9):
        raise ValueError(f'unexpected DHMV window placement: {name}')
    values = dataset.ReadAsArray()
    nodata = dataset.GetRasterBand(1).GetNoDataValue()
    if bool(((values == nodata) | ~np.isfinite(values)).all()):
        target.unlink()
        print(json.dumps({'skipped_sea': name}), flush=True)
        return None
    record = dict(url=METADATA, request_url=url, fetched_utc=utc_now(), sha256=digest(target),
                  bytes=target.stat().st_size, licence=LICENCE, licence_url=LICENCE_URL,
                  terms_checked_utc='2026-09-25', raw_bytes_retained=False,
                  notes=('DHMV II DTM 1 m (ALS 2013-2015) WCS window at 5 m; Belgian Lambert 72 '
                         '(EPSG:31370), Ostend/DNG heights (EPSG:5710); raw 1 m bytes not retained.'))
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
    fetch(output, PROVIDER, 'metadata.xml', METADATA, LICENCE, LICENCE_URL, '2026-09-25',
          'Official DHMV II ISO metadata with the WCS endpoint and licence.')
    windows = grid_windows(*EXTENT, STEP)
    # Populate the central validation region first, without changing full coverage.
    windows.sort(key=lambda w: (w[0] - 130000) ** 2 + (w[1] - 190000) ** 2)
    if args.max_items:
        windows = windows[:args.max_items]
    mask = LandMask(args.land_mask) if args.land_mask else None
    print(json.dumps({'windows': len(windows)}), flush=True)

    def worker(entry):
        position, window = entry
        started = time.monotonic()
        try:
            return window, fetch_window(output, window, mask, position, len(windows))
        finally:
            time.sleep(max(0, 1 - (time.monotonic() - started)))

    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        results = list(pool.map(worker, enumerate(windows, 1)))
    kept = [(window, record) for window, record in results if record]
    sources = [dict(path=str((output / PROVIDER /
                              f'dhmv2_dtm_5m_{window[0]}_{window[1]}.tif').resolve()),
                    horizontal_crs='EPSG:31370', vertical_crs=5710, epoch='ALS 2013-2015',
                    role='national', group='BE-VL-DHMVII', nodata=-9999,
                    datum_area_of_interest=[2.5, 49.4, 6.5, 51.6]) for window, _ in kept]
    publish_source_json(output, output / PROVIDER / 'country-sources.json', sources)
    publish_json(output / PROVIDER / 'skipped-sea.json',
                 sorted(f'{window[0]}_{window[1]}' for window, record in results if not record))
    print(json.dumps({'kept': len(kept), 'skipped': len(windows) - len(kept)}), flush=True)


if __name__ == '__main__':
    main()
