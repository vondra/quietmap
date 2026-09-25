#!/usr/bin/env python3
"""Fetch selected Bavarian DGM5 archive names from the official Metalink catalogue."""
import argparse
import json
from pathlib import Path
import time
import tempfile
import zipfile
import xml.etree.ElementTree as ET
import numpy as np
from osgeo import gdal, osr
from terrain_io import publish_source_json, fetch, provenance, publish_json, digest, source_budget, publish_bytes

gdal.UseExceptions()
osr.UseExceptions()

CATALOGUE = 'https://geodaten.bayern.de/odd/a/dgm/dgm5xyz/meta/metalink/09.meta4'
LICENCE_URL = 'https://creativecommons.org/licenses/by/4.0/'


def decode_archive(path):
    with source_budget(path.parent.parent) as available:
        _decode_archive(path, available)


def _decode_archive(path, available):
    """Decode the verified regular XYZ lattice without resampling or changing its datum."""
    record = provenance(path)
    target = path.with_suffix('.tif')
    if Path(str(target) + '.provenance.json').exists():
        derived = provenance(target)
        if derived.get('parent_sha256') != record['sha256']:
            raise ValueError('derived raster references another source archive')
        return
    with zipfile.ZipFile(path) as archive:
        names = [n for n in archive.namelist() if n == path.with_suffix('.txt').name]
        if len(names) != 1:
            raise ValueError('expected one XYZ lattice in the official archive')
        points = np.loadtxt(archive.open(names[0]))
    xs, ys = np.unique(points[:, 0]), np.unique(points[:, 1])[::-1]
    if len(xs) * len(ys) != len(points) or not np.all(np.diff(xs) == 5) or not np.all(np.diff(ys) == -5):
        raise ValueError('DGM5 source is not a complete 5 m lattice')
    rows = np.rint((ys[0] - points[:, 1]) / 5).astype(int)
    cols = np.rint((points[:, 0] - xs[0]) / 5).astype(int)
    values = np.full((len(ys), len(xs)), np.nan)
    values[rows, cols] = points[:, 2]
    if not np.isfinite(values).all():
        raise ValueError('missing or duplicated DGM5 XYZ nodes')
    # An uncompressed float grid bounds the compressed output, with TIFF/receipt overhead.
    if 2 * values.size * 4 + 131072 > available:
        raise ValueError('decoded terrain would exceed the shared source budget')
    with tempfile.TemporaryDirectory(dir=path.parent) as temp:
        staged = Path(temp) / target.name
        ds = gdal.GetDriverByName('GTiff').Create(str(staged), len(xs), len(ys), 1, gdal.GDT_Float32,
                                               options=['COMPRESS=DEFLATE'])
        crs = osr.SpatialReference(); crs.ImportFromEPSG(25832)
        ds.SetProjection(crs.ExportToWkt()); ds.SetGeoTransform([xs[0]-2.5, 5, 0, ys[0]+2.5, 0, -5])
        ds.GetRasterBand(1).SetNoDataValue(float('nan')); ds.GetRasterBand(1).WriteArray(values)
        ds = None
        if 2 * staged.stat().st_size + 65536 > available:
            raise ValueError('decoded terrain exceeds its reserved peak space')
        publish_bytes(target, staged.read_bytes())
    publish_json(str(target) + '.provenance.json', dict(record, sha256=digest(target), bytes=target.stat().st_size,
                 parent_sha256=record['sha256'], notes='Lossless grid placement of verified DGM5 XYZ; EPSG:25832 + 7837; no resampling.'))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--tile', action='append', default=[], help='Exact archive name from the catalogue; omit to list')
    parser.add_argument('--all', action='store_true', help='Acquire and decode the complete official catalogue')
    args = parser.parse_args()
    provider = 'de-by-dgm5'
    fetch(args.output, provider, 'catalogue.meta4', CATALOGUE, 'CC BY 4.0', LICENCE_URL, '2026-09-24')
    files = ET.parse(args.output / provider / 'catalogue.meta4').findall('.//{*}file')
    wanted = set(args.tile)
    selected = [e for e in files if args.all or e.attrib['name'] in wanted]
    sources = []
    for entry in files:
        name = entry.attrib['name']
        if not wanted and not args.all:
            print(name)
            continue
        if name in wanted or args.all:
            started = time.monotonic()
            url = entry.find('{*}url').text
            sha = entry.find("{*}hash[@type='sha-256']")
            fetch(args.output, provider, name, url, 'CC BY 4.0', LICENCE_URL, '2026-09-24',
                  'Bayerische Vermessungsverwaltung DGM5 XYZ; ETRS89/UTM32, DHHN2016 (EPSG:7837).',
                  expected_sha256=sha.text if sha is not None else None)
            path = args.output / provider / name
            decode_archive(path)
            sources.append(dict(path=str(path.with_suffix('.tif').resolve()), horizontal_crs='EPSG:25832',
                                vertical_crs=7837, epoch='ALS epoch unavailable in download catalogue', group='DE-BY-DGM5', role='national'))
            print(json.dumps(dict(done=len(sources), total=len(selected), path=str(path), seconds=time.monotonic() - started)), flush=True)
            time.sleep(max(0, 1 - (time.monotonic() - started)))
    absent = wanted - {entry.attrib['name'] for entry in files}
    if absent:
        raise ValueError(f'not in official catalogue: {sorted(absent)}')
    if args.all:
        publish_source_json(args.output, args.output / provider / 'country-sources.json', sources)


if __name__ == '__main__':
    main()
