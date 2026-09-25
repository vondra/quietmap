#!/usr/bin/env python3
"""Crop sea-filled GEDTM fallback rasters with coarse land masks per country."""
import argparse
import json
from pathlib import Path

import numpy as np
from osgeo import gdal

from terrain_io import digest, provenance, publish_json, publish_path

gdal.UseExceptions()

PROVIDER = 'gedtm-crops'
# Reviewed lon/lat bounds with margin beyond each national extent for seam halos.
COUNTRY_BBOX = {
    'at': (9.0, 46.0, 17.5, 49.5),
    'ch': (5.5, 45.5, 11.0, 48.2),
    'nl': (2.5, 50.5, 7.5, 54.0),
    'be': (2.0, 49.0, 6.8, 51.8),
    'fr': (-5.5, 41.0, 10.2, 51.5),
    'dk': (7.5, 54.0, 16.0, 58.2),
    'se': (8.5, 54.8, 26.0, 69.5),
    'pt': (-10.0, 36.5, -5.5, 42.5),
}
MASK_DOWNSAMPLE = 4
GEDTM_NODATA = 3.4028235e+38


def parent_gedtm(root):
    candidates = sorted((root / 'gedtm30-v1.2').glob('*.tif'))
    if len(candidates) != 1:
        raise ValueError('expected exactly one retained GEDTM source')
    return candidates[0]


def snap_window(dataset, bbox):
    origin_x, pixel, _, origin_y, _, signed_pixel = dataset.GetGeoTransform()
    pixel_y = -signed_pixel
    west, south, east, north = bbox
    x0 = int((west - origin_x) / pixel)
    y0 = int((origin_y - north) / pixel_y)
    x1 = int(-(-(east - origin_x) // pixel))
    y1 = int(-(-(origin_y - south) // pixel_y))
    if not 0 <= x0 < x1 <= dataset.RasterXSize and 0 <= y0 < y1 <= dataset.RasterYSize:
        raise ValueError('country bounds exceed the GEDTM grid')
    return x0, y0, x1 - x0, y1 - y0


def crop_country(root, code):
    root = Path(root)
    parent = parent_gedtm(root)
    record = provenance(parent)
    dataset = gdal.Open(str(parent))
    x0, y0, columns, rows = snap_window(dataset, COUNTRY_BBOX[code])
    transform = dataset.GetGeoTransform()
    origin = (transform[0] + x0 * transform[1], transform[3] + y0 * transform[5])
    target = root / PROVIDER / f'gedtm30-seafill-{code}.tif'
    mask_target = root / PROVIDER / f'gedtm30-landmask-{code}.tif'
    target.parent.mkdir(parents=True, exist_ok=True)
    driver = gdal.GetDriverByName('GTiff')
    out = driver.Create(str(target) + '.part', columns, rows, 1, gdal.GDT_Float32,
                        options=['COMPRESS=DEFLATE', 'TILED=YES', 'PREDICTOR=2', 'BIGTIFF=YES'])
    out.SetProjection(dataset.GetProjection())
    out.SetGeoTransform((origin[0], transform[1], 0, origin[1], 0, transform[5]))
    # Raw values are metres; the source's erroneous 0.1 scale tag must not propagate.
    out.GetRasterBand(1).SetNoDataValue(GEDTM_NODATA)
    mask_columns = (columns + MASK_DOWNSAMPLE - 1) // MASK_DOWNSAMPLE
    mask_rows = (rows + MASK_DOWNSAMPLE - 1) // MASK_DOWNSAMPLE
    mask = driver.Create(str(mask_target) + '.part', mask_columns, mask_rows, 1, gdal.GDT_Byte,
                         options=['COMPRESS=DEFLATE', 'TILED=YES'])
    mask.SetProjection(dataset.GetProjection())
    mask.SetGeoTransform((origin[0], transform[1] * MASK_DOWNSAMPLE, 0,
                          origin[1], 0, transform[5] * MASK_DOWNSAMPLE))
    nodata_tag = dataset.GetRasterBand(1).GetNoDataValue()
    if nodata_tag is None or not np.isfinite(np.float32(nodata_tag)):
        raise ValueError('GEDTM source lost its nodata tag')
    filled = land = 0
    mask_array = np.zeros((mask_rows, mask_columns), dtype=np.uint8)
    block = 2048
    for row in range(0, rows, block):
        height = min(block, rows - row)
        values = dataset.ReadAsArray(x0, y0 + row, columns, height)
        if values is None or values.shape != (height, columns):
            raise ValueError('GEDTM window read failed')
        # Compare in float32: the nodata tag literal differs from float32 max in float64.
        sea = values == np.float32(nodata_tag)
        filled += int(sea.sum())
        land += int((~sea).sum())
        values[sea] = 0
        out.GetRasterBand(1).WriteArray(values, 0, row)
        # Block rows are multiples of the mask factor, so mask rows never straddle blocks.
        padded = np.zeros((height, mask_columns * MASK_DOWNSAMPLE), dtype=bool)
        padded[:, :columns] = ~sea
        padded = padded.reshape(height, mask_columns, MASK_DOWNSAMPLE).max(axis=2)
        pad_rows = (-height) % MASK_DOWNSAMPLE
        if pad_rows:
            padded = np.vstack([padded, np.zeros((pad_rows, mask_columns), dtype=bool)])
        coarse = padded.reshape(-1, MASK_DOWNSAMPLE, mask_columns).max(axis=1)
        mask_array[row // MASK_DOWNSAMPLE:row // MASK_DOWNSAMPLE + len(coarse)] |= coarse
    out = mask = None
    if not land:
        raise ValueError('country crop holds no GEDTM land')
    mask_dataset = gdal.Open(str(mask_target) + '.part', gdal.GA_Update)
    mask_dataset.GetRasterBand(1).WriteArray(mask_array)
    mask_dataset = None
    for path in (target, mask_target):
        publish_path(path, Path(str(path) + '.part'))
    publish_json(str(target) + '.provenance.json', dict(
        url='derived from retained ' + parent.name, fetched_utc=record['fetched_utc'],
        sha256=digest(target), bytes=target.stat().st_size, licence=record['licence'],
        licence_url=record['licence_url'], terms_checked_utc=record['terms_checked_utc'],
        notes=(f'GEDTM30 v1.2 crop {COUNTRY_BBOX[code]} snapped to the native 1 arc-second grid; '
               f'sea (source nodata) set to 0 m EGM2008 to match the served sea encoding; '
               f'{land} land nodes, {filled} sea nodes; raw metres (scale tag dropped); '
               f'parent sha256 {record["sha256"]}.')))
    publish_json(str(mask_target) + '.provenance.json', dict(
        url='derived from retained ' + parent.name, fetched_utc=record['fetched_utc'],
        sha256=digest(mask_target), bytes=mask_target.stat().st_size, licence=record['licence'],
        licence_url=record['licence_url'], terms_checked_utc=record['terms_checked_utc'],
        notes=(f'Byte land mask at {MASK_DOWNSAMPLE} arc-seconds (1 = any GEDTM land in the cell) '
               f'for download-window pre-filtering; parent sha256 {record["sha256"]}.')))
    return dict(path=str(target.resolve()), horizontal_crs='EPSG:4326', vertical_crs=3855,
                epoch='2006-2015', role='fallback', group='GEDTM30-v1.2-seafill',
                nodata=GEDTM_NODATA, datum_area_of_interest=None,
                land_mask=str(mask_target.resolve()))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--country', choices=sorted(COUNTRY_BBOX), required=True)
    args = parser.parse_args()
    entry = crop_country(args.output, args.country)
    manifest = args.output / PROVIDER / f'fallback-{args.country}.json'
    publish_json(manifest, entry)
    print(json.dumps({'country': args.country, 'path': entry['path']}))


if __name__ == '__main__':
    main()
