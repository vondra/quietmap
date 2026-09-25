#!/usr/bin/env python3
"""Fetch Danmarks Hoejdemodel Terraen as retained 25 m WCS windows (DVR90)."""
import argparse
import json
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import time
import urllib.request

from osgeo import gdal

from dem_windows import LandMask, download_bytes, grid_windows, nodata_tag, reproject_bounds
from terrain_io import digest, provenance, publish_bytes, publish_json, publish_source_json, utc_now

gdal.UseExceptions()

PROVIDER = 'dk-dhm'
WCS = 'https://api.dataforsyningen.dk/dhm_wcs_DAF'
COVERAGE = 'dhm_terraen'
LICENCE = 'CC BY 4.0'
LICENCE_URL = 'https://creativecommons.org/licenses/by/4.0/'
CRS = 25832
RESOLUTION = 25
STEP = 10000
EXTENT = (441000, 6049000, 894000, 6403000)


def read_token(path):
    return Path(path).read_text().strip()


def window_request(x0, y0, x1, y1):
    width, height = (x1 - x0) // RESOLUTION, (y1 - y0) // RESOLUTION
    query = (f'SERVICE=WCS&VERSION=1.0.0&REQUEST=GetCoverage&COVERAGE={COVERAGE}'
             f'&CRS=EPSG:{CRS}&BBOX={x0},{y0},{x1},{y1}&WIDTH={width}&HEIGHT={height}&FORMAT=GTiff')
    return query, width, height


def fetch_window(output, window, mask, token, position, total):
    x0, y0, x1, y1 = window
    name = f'dhm_terraen_25m_{x0}_{y0}'
    target = output / PROVIDER / (name + '.tif')
    receipt = Path(str(target) + '.provenance.json')
    query, width, height = window_request(x0, y0, x1, y1)
    # The access token authenticates the request but must never be retained.
    redacted = WCS + '?' + query + '&token=REDACTED'
    if target.exists() and receipt.exists():
        record = provenance(target)
        if record.get('request_url') != redacted:
            raise ValueError(f'retained window references another request: {target}')
        return window, record
    if mask is not None and not mask.has_land(*reproject_bounds(x0, y0, x1, y1, CRS)):
        return window, None
    started = time.monotonic()
    url = WCS + '?' + query + '&token=' + urllib.request.quote(token)
    payload, _ = download_bytes(url)
    publish_bytes(target, payload)
    dataset = gdal.Open(str(target))
    if dataset is None:
        raise ValueError(f'DHM window is not a raster: {name}: {payload[:200]!r}')
    transform = dataset.GetGeoTransform()
    if (dataset.RasterXSize, dataset.RasterYSize) != (width, height):
        raise ValueError(f'unexpected DHM window grid: {name}')
    if not (abs(transform[0] - x0) < 0.01 and abs(transform[3] - y1) < 0.01
            and abs(transform[1] - RESOLUTION) < 1e-9):
        raise ValueError(f'unexpected DHM window placement: {name}')
    record = dict(url='https://dataforsyningen.dk/data/930', request_url=redacted,
                  fetched_utc=utc_now(), sha256=digest(target),
                  bytes=target.stat().st_size, licence=LICENCE, licence_url=LICENCE_URL,
                  terms_checked_utc='2026-09-25', raw_bytes_retained=False,
                  notes=('DHM Terraen 0.4 m (ALS 2014-2015) WCS window at 25 m, the 1 arc-second '
                         'equivalent; ETRS89/UTM32 (EPSG:25832), DVR90 heights (EPSG:10484 '
                         'DVR90(2013) realization); sea is returned as 0; raw 0.4 m tile bytes '
                         '(854 GB) not retained; attribution Klimadatastyrelsen. Re-fetch with the '
                         'Dataforsyningen API token.'))
    publish_json(receipt, record)
    print(json.dumps(dict(done=position, total=total, path=str(target),
                           seconds=time.monotonic() - started)), flush=True)
    return window, record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--token-file', type=Path, required=True)
    parser.add_argument('--land-mask', type=Path, default=None)
    parser.add_argument('--jobs', type=int, default=4)
    parser.add_argument('--max-items', type=int, default=0)
    args = parser.parse_args()
    if not 1 <= args.jobs <= 8:
        parser.error('jobs must stay within the batch thread budget')
    output = args.output
    token = read_token(args.token_file)
    windows = grid_windows(*EXTENT, STEP)
    windows.sort(key=lambda w: (w[0] - 600000) ** 2 + (w[1] - 6220000) ** 2)
    if args.max_items:
        windows = windows[:args.max_items]
    mask = LandMask(args.land_mask) if args.land_mask else None
    print(json.dumps({'windows': len(windows)}), flush=True)

    def worker(entry):
        position, window = entry
        started = time.monotonic()
        try:
            return fetch_window(output, window, mask, token, position, len(windows))
        finally:
            time.sleep(max(0, 1 - (time.monotonic() - started)))

    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        results = list(pool.map(worker, enumerate(windows, 1)))
    kept = [(window, record) for window, record in results if record]
    tags = {nodata_tag(output / PROVIDER / f'dhm_terraen_25m_{window[0]}_{window[1]}.tif')
            for window, _ in kept}
    if len(tags) != 1:
        raise ValueError(f'mixed nodata tags in DHM windows: {tags}')
    nodata = tags.pop()
    sources = [dict(path=str((output / PROVIDER /
                              f'dhm_terraen_25m_{window[0]}_{window[1]}.tif').resolve()),
                    horizontal_crs='EPSG:25832', vertical_crs=10484, epoch='ALS 2014-2015',
                    role='national', group='DK-DHM', nodata=nodata,
                    datum_area_of_interest=[7.9, 54.4, 15.7, 57.8]) for window, _ in kept]
    publish_source_json(output, output / PROVIDER / 'country-sources.json', sources)
    publish_json(output / PROVIDER / 'skipped-sea.json',
                 sorted(f'{window[0]}_{window[1]}' for window, record in results if not record))
    print(json.dumps({'kept': len(kept), 'skipped': len(windows) - len(kept)}), flush=True)


if __name__ == '__main__':
    main()
