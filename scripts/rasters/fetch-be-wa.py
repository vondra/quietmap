#!/usr/bin/env python3
"""Fetch Wallonie MNT 2021-2022 province zips transiently as retained 5 m tiles."""
import argparse
import http.client
import json
import math
from pathlib import Path
import time
import urllib.error
import urllib.request
import xml.etree.ElementTree as ET
import zipfile

from osgeo import gdal, osr

from dem_windows import nodata_tag
from terrain_io import digest, provenance, publish_json, publish_path, publish_source_json, utc_now

gdal.UseExceptions()
osr.UseExceptions()

PROVIDER = 'be-wa-mnt'
ATOM = ('https://geoservices.wallonie.be/geotraitement/spwdatadownload/results/'
        'a004e570-99d6-4fe5-b83d-49b774409278/atom_dataset.xml')
LICENCE = 'CC BY 4.0'
LICENCE_URL = 'https://creativecommons.org/licenses/by/4.0/'
CRS = 3812
RESOLUTION = 5


def province_zips():
    with urllib.request.urlopen(urllib.request.Request(
            ATOM, headers={'User-Agent': 'QuietMap terrain producer'}), timeout=120) as response:
        root = ET.fromstring(response.read())
    namespace = {'atom': 'http://www.w3.org/2005/Atom'}
    zips = []
    for entry in root.findall('atom:entry', namespace):
        for link in entry.findall('atom:link', namespace):
            href = link.get('href', '')
            if '_3812_PROV_' in href and href.endswith('.zip'):
                zips.append((href.rsplit('/', 1)[1], href, int(link.get('length'))))
    return sorted(set(zips))


def download_transient(url, expected_bytes, scratch, attempts=3):
    scratch.mkdir(parents=True, exist_ok=True)
    target = scratch / url.rsplit('/', 1)[1]
    if target.exists() and target.stat().st_size == expected_bytes:
        return target
    if target.exists():
        target.unlink()
    request = urllib.request.Request(url, headers={'User-Agent': 'QuietMap terrain producer'})
    last = None
    for attempt in range(attempts):
        # A 50 GB province pull always trips a transient stall (IncompleteRead
        # killed the Luxembourg pull 2026-09-26), so restart the pull, not the run.
        try:
            with urllib.request.urlopen(request, timeout=600) as response, open(target, 'wb') as out:
                size = 0
                while block := response.read(1 << 20):
                    size += len(block)
                    out.write(block)
                if size != expected_bytes:
                    raise ValueError(f'incomplete province archive: {target}')
            return target
        except (http.client.IncompleteRead, http.client.RemoteDisconnected,
                TimeoutError, ConnectionError, urllib.error.URLError) as error:
            if isinstance(error, urllib.error.HTTPError) and error.code < 500 and error.code != 429:
                raise
            if attempt + 1 == attempts:
                raise
            last = error
        time.sleep(2 ** attempt)
    raise last


def reduce_province(archive, member, output, record):
    dataset = gdal.Open(f'/vsizip/{archive}/{member}')
    if dataset is None or dataset.RasterCount != 1:
        raise ValueError(f'expected one band: {member}')
    transform = dataset.GetGeoTransform()
    if transform[2] or transform[4] or abs(transform[1] - 0.5) > 1e-12 or abs(transform[5] + 0.5) > 1e-12:
        raise ValueError(f'expected a north-up 0.5 m grid: {member}')
    file_crs = osr.SpatialReference()
    file_crs.ImportFromWkt(dataset.GetProjection())
    reference = osr.SpatialReference()
    reference.ImportFromEPSG(CRS)
    if not file_crs.IsSame(reference):
        raise ValueError(f'expected Lambert 2008 parameters: {member}')
    nodata = dataset.GetRasterBand(1).GetNoDataValue()
    if nodata is None:
        raise ValueError(f'province file lacks a nodata tag: {member}')
    # Snap out to absolute 5 m multiples: every province lands on one shared grid.
    x0 = math.floor(transform[0] / RESOLUTION) * RESOLUTION
    y1 = math.ceil(transform[3] / RESOLUTION) * RESOLUTION
    x1 = math.ceil((transform[0] + dataset.RasterXSize * transform[1]) / RESOLUTION) * RESOLUTION
    y0 = math.floor((transform[3] + dataset.RasterYSize * transform[5]) / RESOLUTION) * RESOLUTION
    name = archive.stem.replace('_3812_PROV_', '_5m_') + '.tif'
    target = output / PROVIDER / name
    receipt = Path(str(target) + '.provenance.json')
    target.parent.mkdir(parents=True, exist_ok=True)
    if target.exists() and receipt.exists():
        kept = provenance(target)
        if kept.get('parent_sha256') != record['sha256']:
            raise ValueError(f'retained tile references another archive: {target}')
        return kept
    warped = gdal.Warp(str(target) + '.part', dataset, format='GTiff', dstSRS=f'EPSG:{CRS}',
                       xRes=RESOLUTION, yRes=RESOLUTION, outputBounds=(x0, y0, x1, y1),
                       resampleAlg='average', srcNodata=nodata, dstNodata=nodata,
                       outputType=gdal.GDT_Float32,
                       creationOptions=['COMPRESS=DEFLATE', 'TILED=YES', 'PREDICTOR=2'],
                       multithread=True, warpOptions=['NUM_THREADS=2', 'WarpMemoryLimit=4096'])
    if warped is None:
        raise ValueError(f'province reduction failed: {member}')
    warped = None
    publish_path(target, Path(str(target) + '.part'))
    kept = dict(url=record['url'], fetched_utc=utc_now(), sha256=digest(target),
                bytes=target.stat().st_size, licence=LICENCE, licence_url=LICENCE_URL,
                terms_checked_utc='2026-09-25', parent_sha256=record['sha256'],
                parent_bytes=record['bytes'], parent_member=member, raw_bytes_retained=False,
                notes=('Wallonie MNT 2021-2022 0.5 m province file area-averaged to 5 m on the '
                       'absolute 5 m Lambert 2008 grid (bounds snapped out); ETRS89 Lambert 2008 '
                       '(EPSG:3812), Ostend/DNG heights (EPSG:5710); raw province bytes (212 GB '
                       'total) not retained.'))
    publish_json(receipt, kept)
    return kept


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--scratch-dir', type=Path, default=None)
    parser.add_argument('--provinces', nargs='+', default=None)
    args = parser.parse_args()
    output = args.output
    scratch = args.scratch_dir or output.parent / 'dem-transient'
    selected = province_zips()
    if args.provinces:
        selected = [entry for entry in selected
                    if any(tag in entry[0] for tag in args.provinces)]
        if not selected:
            parser.error('no province archive matches the filter')
    print(json.dumps({'archives': [name for name, _, _ in selected]}), flush=True)
    for name, url, expected in selected:
        started = time.monotonic()
        archive = download_transient(url, expected, scratch / PROVIDER)
        record = dict(url=url, sha256=digest(archive), bytes=archive.stat().st_size)
        print(json.dumps({'archive': name, 'sha256': record['sha256']}), flush=True)
        with zipfile.ZipFile(archive) as container:
            members = sorted(n for n in container.namelist() if n.endswith('.tif'))
        if len(members) != 1:
            raise ValueError(f'expected one province raster in {name}: {members}')
        reduce_province(archive, members[0], output, record)
        archive.unlink()
        print(json.dumps({'archive': name, 'seconds': time.monotonic() - started}), flush=True)
    sources = []
    nodata_values = set()
    for target in sorted((output / PROVIDER).glob('*_5m_*.tif')):
        if not Path(str(target) + '.provenance.json').exists():
            raise ValueError(f'reduced tile lacks a receipt: {target}')
        nodata_values.add(nodata_tag(target))
    if len(nodata_values) != 1:
        raise ValueError(f'mixed nodata tags in reduced tiles: {nodata_values}')
    nodata = nodata_values.pop()
    for target in sorted((output / PROVIDER).glob('*_5m_*.tif')):
        sources.append(dict(path=str(target.resolve()), horizontal_crs='EPSG:3812',
                            vertical_crs=5710, epoch='ALS 2021-2022', role='national',
                            group='BE-WA-MNT', nodata=nodata,
                            datum_area_of_interest=[2.5, 49.4, 6.5, 51.6]))
    publish_source_json(output, output / PROVIDER / 'country-sources.json', sources)
    print(json.dumps({'kept': len(sources)}), flush=True)


if __name__ == '__main__':
    main()
