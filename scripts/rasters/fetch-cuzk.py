#!/usr/bin/env python3
"""Fetch bounded official DMR5G ground windows with a recorded native Bpv datum."""
import argparse
import math
import json
from pathlib import Path
import time
from osgeo import gdal
from urllib.parse import urlencode
from terrain_io import publish_source_json, fetch

SERVICE = 'https://ags.cuzk.gov.cz/arcgis2/rest/services/dmr5g/ImageServer'
LICENCE = 'https://ags.cuzk.gov.cz/opendata/'


def country(output):
    """Retain aligned 5 m service rasters over the advertised complete native extent."""
    provider = 'cuzk-dmr5g'
    fetch(output, provider, 'service.json', SERVICE + '?f=pjson', 'CC BY 4.0', LICENCE, '2026-09-24')
    metadata = json.loads((output / provider / 'service.json').read_text())
    extent = metadata['extent']
    if extent['spatialReference']['latestWkid'] != 5514 or extent['spatialReference']['latestVcsWkid'] != 8357:
        raise ValueError('official service horizontal or vertical datum changed')
    tiles = [(x, y) for x in range(math.floor(extent['xmin'] / 10000), math.ceil(extent['xmax'] / 10000))
             for y in range(math.floor(extent['ymin'] / 10000), math.ceil(extent['ymax'] / 10000))]
    # Populate the central validation region first, without changing complete-country coverage.
    tiles.sort(key=lambda xy: (xy[0] + 75) ** 2 + (xy[1] + 107) ** 2)
    sources = []
    for index, (x, y) in enumerate(tiles):
        started = time.monotonic()
        bbox = [x * 10000, y * 10000, (x + 1) * 10000, (y + 1) * 10000]
        query = urlencode(dict(bbox=','.join(map(str, bbox)), bboxSR=5514, imageSR=5514,
                               size='2000,2000', format='tiff', pixelType='F32', noData=-9999,
                               renderingRule='{"rasterFunction":"None"}',
                               interpolation='RSP_BilinearInterpolation', f='image'))
        name = f'dmr5g_5m_{x}_{y}.tif'
        fetch(output, provider, name, SERVICE + '/exportImage?' + query, 'CC BY 4.0', LICENCE,
              '2026-09-24', 'DMR5G ALS 2009-2013, Bpv EPSG:8357; native aligned 5 m service export; raw bytes retained.')
        path = output / provider / name
        dataset = gdal.Open(str(path))
        if dataset is None or dataset.RasterXSize != 2000 or dataset.RasterYSize != 2000:
            raise ValueError(f'invalid service raster: {path}')
        sources.append(dict(path=str(path.resolve()), horizontal_crs='EPSG:5514', vertical_crs=8357,
                            epoch='2009-2013', role='national', group='CZ-DMR5G'))
        print(json.dumps(dict(done=index + 1, total=len(tiles), path=str(path), seconds=time.monotonic() - started)), flush=True)
        time.sleep(max(0, 1 - (time.monotonic() - started)))
    publish_source_json(output, output / provider / 'country-sources.json', sources)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output')
    scope = parser.add_mutually_exclusive_group(required=True)
    scope.add_argument('--bbox', nargs=4, type=float, metavar=('W', 'S', 'E', 'N'))
    scope.add_argument('--country', action='store_true')
    args = parser.parse_args()
    if args.country:
        country(Path(args.output))
        return
    west, south, east, north = args.bbox
    # A 5 m geographic request retains the official ground grid, not a shaded image.
    columns = math.ceil((east - west) * 111320 * math.cos(math.radians((north + south) / 2)) / 5)
    rows = math.ceil((north - south) * 110540 / 5)
    if not (1 <= columns <= 4100 and 1 <= rows <= 4100):
        parser.error('split the bounds into windows of at most 4100 pixels per side')
    query = urlencode(dict(bbox=','.join(map(str, args.bbox)), bboxSR=4326, imageSR=4326,
                           size=f'{columns},{rows}', format='tiff', pixelType='F32', noData=-9999,
                           renderingRule='{"rasterFunction":"None"}',
                           interpolation='RSP_BilinearInterpolation', f='image'))
    url = 'https://ags.cuzk.cz/arcgis2/rest/services/dmr5g/ImageServer/exportImage?' + query
    name = f'dmr5g_{west}_{south}_{east}_{north}.tif'
    fetch(args.output, 'cuzk-dmr5g', name, url, 'CC BY 4.0',
          'https://creativecommons.org/licenses/by/4.0/', '2026-09-24',
          'Official 5 m DMR5G service grid, ALS 2009-2013; raw metres, Bpv (EPSG:8357); no vertical conversion on fetch.')


if __name__ == '__main__':
    main()
