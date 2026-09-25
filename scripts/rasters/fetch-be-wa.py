#!/usr/bin/env python3
"""Fetch Wallonie MNT 2021-2022 province zips transiently as retained 5 m tiles."""
import argparse
import hashlib
import json
from pathlib import Path
import time
import urllib.request
import xml.etree.ElementTree as ET
import zipfile

import numpy as np
from osgeo import gdal, osr

from terrain_io import digest, provenance, publish_json, publish_source_json, utc_now

gdal.UseExceptions()
osr.UseExceptions()

PROVIDER = 'be-wa-mnt'
ATOM = ('https://geoservices.wallonie.be/geotraitement/spwdatadownload/results/'
        'a004e570-99d6-4fe5-b83d-49b774409278/atom_dataset.xml')
LICENCE = 'CC BY 4.0'
LICENCE_URL = 'https://creativecommons.org/licenses/by/4.0/'
CRS = 3812
FACTOR = 10


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


def download_transient(url, expected_bytes, scratch):
    scratch.mkdir(parents=True, exist_ok=True)
    target = scratch / url.rsplit('/', 1)[1]
    if target.exists() and target.stat().st_size == expected_bytes:
        return target
    if target.exists():
        target.unlink()
    checksum = hashlib.sha256()
    size = 0
    request = urllib.request.Request(url, headers={'User-Agent': 'QuietMap terrain producer'})
    with urllib.request.urlopen(request, timeout=600) as response, open(target, 'wb') as out:
        while block := response.read(1 << 20):
            size += len(block)
            checksum.update(block)
            out.write(block)
    if size != expected_bytes:
        raise ValueError(f'incomplete province archive: {target}')
    return target


def reduce_member(archive, member, output, record):
    dataset = gdal.Open(f'/vsizip/{archive}/{member}')
    if dataset is None or dataset.RasterCount != 1:
        raise ValueError(f'expected one band: {member}')
    transform = dataset.GetGeoTransform()
    if transform[2] or transform[4] or abs(transform[1] - 0.5) > 1e-12 or abs(transform[5] + 0.5) > 1e-12:
        raise ValueError(f'expected a north-up 0.5 m grid: {member}')
    if abs(transform[0] / 5 - round(transform[0] / 5)) > 1e-6 or abs(transform[3] / 5 - round(transform[3] / 5)) > 1e-6:
        raise ValueError(f'tile origin breaks the 5 m mosaic grid: {member}')
    reference = osr.SpatialReference()
    reference.ImportFromWkt(dataset.GetProjection())
    if reference.GetAuthorityCode('PROJCRS') != str(CRS):
        raise ValueError(f'expected EPSG:{CRS}: {member}')
    rows, columns = dataset.RasterYSize, dataset.RasterXSize
    if rows % FACTOR or columns % FACTOR:
        raise ValueError(f'tile size breaks exact 10x reduction: {member}')
    nodata = dataset.GetRasterBand(1).GetNoDataValue()
    if nodata is None:
        raise ValueError(f'tile lacks a nodata tag: {member}')
    values = dataset.ReadAsArray().astype(np.float64)
    valid = (values != nodata) & np.isfinite(values)
    grouped = (values * valid).reshape(rows // FACTOR, FACTOR, columns // FACTOR, FACTOR)
    counts = valid.reshape(rows // FACTOR, FACTOR, columns // FACTOR, FACTOR).sum(axis=(1, 3))
    with np.errstate(invalid='ignore', divide='ignore'):
        reduced = np.where(counts, grouped.sum(axis=(1, 3)) / np.maximum(counts, 1), nodata)
    name = Path(member).stem + '_5m.tif'
    target = output / PROVIDER / name
    receipt = Path(str(target) + '.provenance.json')
    if target.exists() and receipt.exists():
        kept = provenance(target)
        if kept.get('parent_sha256') != record['sha256']:
            raise ValueError(f'retained tile references another archive: {target}')
        return kept
    driver = gdal.GetDriverByName('GTiff')
    out = driver.Create(str(target), columns // FACTOR, rows // FACTOR, 1, gdal.GDT_Float32,
                        options=['COMPRESS=DEFLATE', 'TILED=YES', 'PREDICTOR=2'])
    out.SetProjection(dataset.GetProjection())
    out.SetGeoTransform((transform[0], transform[1] * FACTOR, 0,
                         transform[3], 0, transform[5] * FACTOR))
    out.GetRasterBand(1).SetNoDataValue(nodata)
    out.GetRasterBand(1).WriteArray(reduced.astype(np.float32))
    out = None
    kept = dict(url=record['url'], fetched_utc=utc_now(), sha256=digest(target),
                bytes=target.stat().st_size, licence=LICENCE, licence_url=LICENCE_URL,
                terms_checked_utc='2026-09-25', parent_sha256=record['sha256'],
                parent_bytes=record['bytes'], parent_member=member, raw_bytes_retained=False,
                notes=('Wallonie MNT 2021-2022 0.5 m tile reduced by exact 10x block means to 5 m; '
                       'ETRS89 Lambert 2008 (EPSG:3812), Ostend/DNG heights (EPSG:5710); raw '
                       'province bytes (212 GB total) not retained.'))
    publish_json(receipt, kept)
    return kept


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--scratch-dir', type=Path, default=None)
    parser.add_argument('--provinces', nargs='+', default=None)
    parser.add_argument('--max-tiles', type=int, default=0)
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
        if args.max_tiles:
            members = members[:args.max_tiles]
        for position, member in enumerate(members, 1):
            reduce_member(archive, member, output, record)
            if position % 50 == 0:
                print(json.dumps({'archive': name, 'done': position, 'total': len(members)}),
                      flush=True)
        archive.unlink()
        print(json.dumps({'archive': name, 'tiles': len(members),
                          'seconds': time.monotonic() - started}), flush=True)
    sources = []
    nodata_values = set()
    for target in sorted((output / PROVIDER).glob('*_5m.tif')):
        if not Path(str(target) + '.provenance.json').exists():
            raise ValueError(f'reduced tile lacks a receipt: {target}')
        nodata_values.add(gdal.Open(str(target)).GetRasterBand(1).GetNoDataValue())
    if len(nodata_values) != 1:
        raise ValueError(f'mixed nodata tags in reduced tiles: {nodata_values}')
    nodata = nodata_values.pop()
    for target in sorted((output / PROVIDER).glob('*_5m.tif')):
        sources.append(dict(path=str(target.resolve()), horizontal_crs='EPSG:3812',
                            vertical_crs=5710, epoch='ALS 2021-2022', role='national',
                            group='BE-WA-MNT', nodata=nodata,
                            datum_area_of_interest=[2.5, 49.4, 6.5, 51.6]))
    publish_source_json(output, output / PROVIDER / 'country-sources.json', sources)
    print(json.dumps({'kept': len(sources)}), flush=True)


if __name__ == '__main__':
    main()
