#!/usr/bin/env python3
"""Fetch bounded official DMR5G ground windows with a recorded native Bpv datum."""
import argparse
import math
from urllib.parse import urlencode
from terrain_io import fetch


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output')
    parser.add_argument('--bbox', nargs=4, type=float, required=True, metavar=('W', 'S', 'E', 'N'))
    args = parser.parse_args()
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
