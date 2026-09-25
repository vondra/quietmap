#!/usr/bin/env python3
"""Plan and produce EU DEM squares from fallback crops and national sources.

A square is produced only when it holds national land: at least one sample
with finite national height on a GEDTM land-mask cell. Pure fallback squares
stay for the world assembly, so a later national rerun is never blocked.
"""
import argparse
import json
import math
from pathlib import Path

import numpy as np
from osgeo import gdal, osr

from dem_windows import square_bounds, square_of_latitude, square_of_longitude
from terrain_produce import produce

gdal.UseExceptions()

Z9 = 512
OWNERSHIP_SAMPLES = 64

COUNTRIES = ('at', 'ch', 'nl', 'be-vl', 'be-wa', 'fr', 'dk', 'se', 'pt')
PROVIDERS = {'at': 'at-dgm10', 'ch': 'ch-alti3d', 'nl': 'nl-ahn4', 'be-vl': 'be-vl-dhmv',
             'be-wa': 'be-wa-mnt', 'fr': 'fr-rgealti', 'dk': 'dk-dhm', 'se': 'se-lm',
             'pt': 'pt-dgt'}
FALLBACK_OF = {'at': 'at', 'ch': 'ch', 'nl': 'nl', 'be-vl': 'be', 'be-wa': 'be',
               'fr': 'fr', 'dk': 'dk', 'se': 'se', 'pt': 'pt'}


def file_lonlat_bounds(path):
    dataset = gdal.Open(str(path))
    source = osr.SpatialReference()
    source.ImportFromWkt(dataset.GetProjection())
    source.SetAxisMappingStrategy(osr.OAMS_TRADITIONAL_GIS_ORDER)
    target = osr.SpatialReference()
    target.ImportFromEPSG(4326)
    target.SetAxisMappingStrategy(osr.OAMS_TRADITIONAL_GIS_ORDER)
    transform = osr.CoordinateTransformation(source, target)
    geo = dataset.GetGeoTransform()
    corners = [(0, 0), (dataset.RasterXSize, 0),
               (dataset.RasterXSize, dataset.RasterYSize), (0, dataset.RasterYSize)]
    projected = [(geo[0] + c * geo[1] + r * geo[2], geo[3] + c * geo[4] + r * geo[5])
                 for c, r in corners]
    converted = transform.TransformPoints(projected)
    longitudes = [p[0] for p in converted]
    latitudes = [p[1] for p in converted]
    return (min(longitudes), min(latitudes), max(longitudes), max(latitudes))


def _intersects(a, b):
    return a[0] < b[2] and a[2] > b[0] and a[1] < b[3] and a[3] > b[1]


def candidate_squares(sources):
    squares = set()
    for source in sources:
        west, south, east, north = file_lonlat_bounds(source['path'])
        for x in range(square_of_longitude(west), square_of_longitude(east) + 1):
            for y in range(square_of_latitude(north), square_of_latitude(south) + 1):
                squares.add((x % Z9, y))
    return squares


def group_vrt(sources, group):
    members = [s['path'] for s in sources if s['group'] == group]
    first = next(s for s in sources if s['group'] == group)
    options = dict(VRTNodata=np.nan, strict=True) if first.get('nodata') is not None else dict(strict=True)
    if first.get('nodata') is not None:
        options['srcNodata'] = first['nodata']
    vrt = gdal.BuildVRT('', members, **options)
    if vrt is None:
        raise ValueError(f'cannot build ownership VRT for {group}')
    return vrt


def owns_square(vrt, mask, bounds):
    west, south, east, north = bounds
    national = gdal.Warp('', vrt, format='MEM', dstSRS='EPSG:4326', outputType=gdal.GDT_Float32,
                         outputBounds=[west, south, east, north],
                         width=OWNERSHIP_SAMPLES, height=OWNERSHIP_SAMPLES,
                         resampleAlg='near', srcNodata=np.nan, dstNodata=np.nan)
    land = gdal.Warp('', mask, format='MEM', dstSRS='EPSG:4326', outputType=gdal.GDT_Byte,
                     outputBounds=[west, south, east, north],
                     width=OWNERSHIP_SAMPLES, height=OWNERSHIP_SAMPLES,
                     resampleAlg='near', srcNodata=255, dstNodata=255)
    heights, cells = national.ReadAsArray(), land.ReadAsArray()
    return bool((np.isfinite(heights) & (cells == 1)).any())


def plan(source_root, countries, work_dir):
    source_root, work_dir = Path(source_root), Path(work_dir)
    work_dir.mkdir(parents=True, exist_ok=True)
    # PROJ caches grid availability, so the search paths must be set before the
    # first transform (ownership touches compound-CRS files whose vertical
    # operations need these grids); a lookup without them poisons later ones.
    grids = sorted((source_root / 'proj-grids').glob('*.tif'))
    if not grids:
        raise ValueError('no PROJ datum grids retained')
    osr.SetPROJSearchPaths(osr.GetPROJSearchPaths() + [str((source_root / 'proj-grids').resolve())])
    fallback, national, masks, per_country = [], [], {}, {}
    for code in countries:
        entry = json.loads((source_root / 'gedtm-crops' / f'fallback-{FALLBACK_OF[code]}.json').read_text())
        if entry not in fallback:
            fallback.append(entry)
        masks[code] = str(source_root / 'gedtm-crops' /
                           f'gedtm30-landmask-{FALLBACK_OF[code]}.tif')
        own = json.loads((source_root / PROVIDERS[code] / 'country-sources.json').read_text())
        per_country[code] = own
        national.extend(own)
    squares = set()
    for code in countries:
        own = per_country[code]
        groups = sorted({s['group'] for s in own})
        mask = gdal.Open(masks[code])
        for group in groups:
            members = [s for s in own if s['group'] == group]
            bounds = [file_lonlat_bounds(s['path']) for s in members]
            west = min(b[0] for b in bounds)
            south = min(b[1] for b in bounds)
            east = max(b[2] for b in bounds)
            north = max(b[3] for b in bounds)
            candidates = [square for square in candidate_squares(members)
                          if _intersects(square_bounds(*square), (west, south, east, north))]
            if not candidates:
                continue
            vrt = group_vrt(members, group)
            for square in candidates:
                if square not in squares and owns_square(vrt, mask, square_bounds(*square)):
                    squares.add(square)
                    print(json.dumps({'owns': square, 'group': group}), flush=True)
    return dict(channel='dem', sources=fallback + national,
                squares=sorted(squares), geoid_grids=[str(p) for p in grids],
                feather_halo_nodes=256)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source-root', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--work-dir', type=Path, required=True)
    parser.add_argument('--raster-repack', type=Path, required=True)
    parser.add_argument('--reserve-bytes', type=int, required=True)
    parser.add_argument('--countries', nargs='+', choices=COUNTRIES, default=list(COUNTRIES))
    parser.add_argument('--max-squares', type=int, default=0)
    args = parser.parse_args()
    manifest = plan(args.source_root, args.countries, args.work_dir)
    if args.max_squares:
        manifest['squares'] = manifest['squares'][:args.max_squares]
    manifest_path = args.work_dir / 'eu-dem-manifest.json'
    manifest_path.write_text(json.dumps(manifest, indent=2) + '\n')
    print(json.dumps({'squares': len(manifest['squares']),
                      'sources': len(manifest['sources'])}), flush=True)
    produce(manifest_path, args.output, args.raster_repack, args.reserve_bytes)


if __name__ == '__main__':
    main()
