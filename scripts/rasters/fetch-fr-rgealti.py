#!/usr/bin/env python3
"""Fetch IGN RGE ALTI as retained 10 m WMS windows (mainland IGN69, Corsica IGN78).

The HIGHRES service renders 5 m 10 km windows in 30-200 s each (measured
2026-09-25), which puts full 5 m coverage beyond reach; 10 m windows take
about 8 s and still oversample the 1 arc-second output grid.

The service blends nodata into coastal pixels across the full continuum
(-99999 down to -10 observed 2026-09-26), so kept windows scrub sub-floor
cells back to nodata before their receipts are written.
"""
import argparse
import json
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import time
import urllib.request

import numpy as np
from osgeo import gdal

from dem_windows import (LandMask, download_bytes, grid_windows, nodata_tag, polite_sleep,
                         reproject_bounds)
from terrain_io import digest, provenance, publish_bytes, publish_json, publish_source_json, utc_now

gdal.UseExceptions()

PROVIDER = 'fr-rgealti'
WMS = 'https://data.geopf.fr/wms-r/wms'
LAYER = 'ELEVATION.ELEVATIONGRIDCOVERAGE.HIGHRES'
LICENCE = 'Licence Ouverte 2.0 (Etalab)'
LICENCE_URL = 'https://www.etalab.gouv.fr/licence-ouverte-open-licence/'
CRS = 2154
RESOLUTION = 10
# No bare land in France sits below -4 m (Les Moëres polder, -2.5 m), while
# Géoplateforme blends nodata into coastal pixels across the full continuum
# (-99999 down to -10 observed 2026-09-26); anything under the floor is blend
# garbage, never terrain.
FR_FLOOR_M = -5.
STEP = 10000
EXTENT = (0, 6020000, 1260000, 7130000)
# Corsica sits 65 km east of the mainland edge; the sea gap keeps the groups disjoint.
CORSICA_X0 = 1090000
CORSICA_Y1 = 6190000


def group_of(x0, y1):
    if x0 >= CORSICA_X0 and y1 <= CORSICA_Y1:
        return 'FR-RGEALTI-CORSE'
    return 'FR-RGEALTI'


def scrub_window(path):
    """Reset sub-floor blend garbage to nodata in place; returns the count."""
    dataset = gdal.Open(str(path), gdal.GA_Update)
    band = dataset.GetRasterBand(1)
    values = band.ReadAsArray()
    garbage = values < FR_FLOOR_M
    count = int(garbage.sum())
    if count:
        values[garbage] = band.GetNoDataValue()
        band.WriteArray(values)
        band.FlushCache()
    dataset = None
    return count


def fetch_window(output, window, mask, position, total):
    x0, y0, x1, y1 = window
    name = f'rgealti_10m_{x0}_{y0}'
    target = output / PROVIDER / (name + '.tif')
    receipt = Path(str(target) + '.provenance.json')
    width, height = (x1 - x0) // RESOLUTION, (y1 - y0) // RESOLUTION
    url = (f'{WMS}?SERVICE=WMS&VERSION=1.3.0&REQUEST=GetMap&LAYERS={LAYER}&STYLES='
           f'&CRS=EPSG:{CRS}&BBOX={x0},{y0},{x1},{y1}&WIDTH={width}&HEIGHT={height}&FORMAT=image/geotiff')
    if target.exists() and receipt.exists():
        record = provenance(target)
        if record.get('request_url') != url:
            raise ValueError(f'retained window references another request: {target}')
        return window, record
    if mask is not None and not mask.has_land(*reproject_bounds(x0, y0, x1, y1, CRS)):
        return window, None
    started = time.monotonic()
    # Six attempts ride out the sustained-load 400 bursts that killed two
    # full-country runs (2026-09-25); a genuinely bad window still raises.
    payload, _ = download_bytes(url, attempts=6)
    publish_bytes(target, payload)
    dataset = gdal.Open(str(target))
    if dataset is None:
        raise ValueError(f'RGE ALTI window is not a raster: {name}: {payload[:200]!r}')
    transform = dataset.GetGeoTransform()
    if (dataset.RasterXSize, dataset.RasterYSize) != (width, height):
        raise ValueError(f'unexpected RGE ALTI window grid: {name}')
    if not (abs(transform[0] - x0) < 0.01 and abs(transform[3] - y1) < 0.01
            and abs(transform[1] - RESOLUTION) < 1e-9):
        raise ValueError(f'unexpected RGE ALTI window placement: {name}')
    values = dataset.ReadAsArray()
    nodata = dataset.GetRasterBand(1).GetNoDataValue()
    if bool(((values == nodata) | ~np.isfinite(values)).all()):
        target.unlink()
        print(json.dumps({'skipped_sea': name}), flush=True)
        polite_sleep(started)
        return window, None
    dataset = None
    scrub_window(target)
    record = dict(url='https://cartes.gouv.fr/rechercher-une-donnee/dataset/IGNF_RGE-ALTI',
                  request_url=url, fetched_utc=utc_now(), sha256=digest(target),
                  bytes=target.stat().st_size, licence=LICENCE, licence_url=LICENCE_URL,
                  terms_checked_utc='2026-09-25', raw_bytes_retained=False,
                  notes=('RGE ALTI (2009-2021 mosaic) WMS HIGHRES window at 10 m; Lambert-93 '
                         '(EPSG:2154); mainland NGF-IGN69 (EPSG:5720), Corsica NGF-IGN78 (EPSG:5721); '
                         'raw tile bytes not retained.'))
    publish_json(receipt, record)
    print(json.dumps(dict(done=position, total=total, path=str(target),
                           seconds=time.monotonic() - started)), flush=True)
    polite_sleep(started)
    return window, record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--land-mask', type=Path, default=None)
    parser.add_argument('--jobs', type=int, default=4)
    parser.add_argument('--max-items', type=int, default=0)
    parser.add_argument('--group', choices=('FR-RGEALTI', 'FR-RGEALTI-CORSE'), default=None)
    args = parser.parse_args()
    if not 1 <= args.jobs <= 8:
        parser.error('jobs must stay within the batch thread budget')
    output = args.output
    windows = grid_windows(*EXTENT, STEP)
    if args.group:
        windows = [w for w in windows if group_of(w[0], w[3]) == args.group]
    windows.sort(key=lambda w: (w[0] - 650000) ** 2 + (w[1] - 6860000) ** 2)
    if args.max_items:
        windows = windows[:args.max_items]
    mask = LandMask(args.land_mask) if args.land_mask else None
    print(json.dumps({'windows': len(windows)}), flush=True)

    def worker(entry):
        position, window = entry
        return fetch_window(output, window, mask, position, len(windows))

    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        results = list(pool.map(worker, enumerate(windows, 1)))
    kept = [(window, record) for window, record in results if record]
    groups = {'FR-RGEALTI': (5720, [-5.2, 41.3, 10.0, 51.2]),
              'FR-RGEALTI-CORSE': (5721, [8.1, 41.3, 9.9, 43.1])}
    tags = {nodata_tag(output / PROVIDER / f'rgealti_10m_{window[0]}_{window[1]}.tif')
            for window, _ in kept}
    if len(tags) != 1:
        raise ValueError(f'mixed nodata tags in RGE ALTI windows: {tags}')
    nodata = tags.pop()
    sources = []
    for window, _ in sorted(kept, key=lambda entry: group_of(entry[0][0], entry[0][3])):
        vertical, aoi = groups[group_of(window[0], window[3])]
        sources.append(dict(path=str((output / PROVIDER /
                                      f'rgealti_10m_{window[0]}_{window[1]}.tif').resolve()),
                            horizontal_crs='EPSG:2154', vertical_crs=vertical,
                            epoch='RGE ALTI 2009-2021 mosaic', role='national',
                            group=group_of(window[0], window[3]), nodata=nodata,
                            datum_area_of_interest=aoi))
    publish_source_json(output, output / PROVIDER / 'country-sources.json', sources)
    publish_json(output / PROVIDER / 'skipped-sea.json',
                 sorted(f'{window[0]}_{window[1]}' for window, record in results if not record))
    print(json.dumps({'kept': len(kept), 'skipped': len(windows) - len(kept)}), flush=True)


if __name__ == '__main__':
    main()
