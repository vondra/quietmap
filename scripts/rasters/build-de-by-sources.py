"""List the base-branch Bavarian DGM5 grids as one national source group.

The BY tiles predate the Länder fetchers and have no stream journal; this
lists them manifest-ready after verifying every tile carries its provenance
sidecar and shares the group's native pixel grid. Partial border slivers
whose origin sits off the 5 m lattice are skipped and reported; the
producer's fallback covers those fragments.
"""
import argparse
import json
from pathlib import Path

from osgeo import gdal, osr

from terrain_io import publish_json

PROVIDER = 'de-by-dgm5'
GROUP = 'DE-BY-DGM5'
HORIZONTAL_EPSG = 25832
VERTICAL_EPSG = 7837
EPOCH = 'ALS epoch not stated; served as the current DGM5 mosaic'
STEP = 5.0


def grid_of(path):
    dataset = gdal.Open(str(path))
    if dataset.RasterCount != 1:
        raise ValueError(f'{path}: expected one band')
    transform = dataset.GetGeoTransform()
    if transform[2] or transform[4] or transform[1] != STEP or transform[5] != -STEP:
        raise ValueError(f'{path}: not a north-up {STEP} m grid')
    crs = osr.SpatialReference(wkt=dataset.GetProjection())
    if crs.GetAuthorityCode('PROJCS') != str(HORIZONTAL_EPSG):
        raise ValueError(f'{path}: horizontal CRS is not EPSG:{HORIZONTAL_EPSG}')
    return transform[0], transform[3]


def build(root):
    names = sorted(p.name for p in (Path(root) / PROVIDER).glob('*.tif'))
    if not names:
        raise ValueError(f'{PROVIDER}: no grids to list')
    entries = []
    skipped = []
    for name in names:
        path = Path(root) / PROVIDER / name
        if not Path(str(path) + '.provenance.json').is_file():
            raise ValueError(f'missing source provenance: {path}')
        west, north = grid_of(path)
        if (abs(west - round(west / STEP) * STEP) > 1e-6
                or abs(north - round(north / STEP) * STEP) > 1e-6):
            skipped.append(name)
            continue
        entries.append(dict(path=str(path.resolve()), horizontal_crs=f'EPSG:{HORIZONTAL_EPSG}',
                            vertical_crs=VERTICAL_EPSG, epoch=EPOCH,
                            role='national', group=GROUP))
    listing = Path(root) / PROVIDER / 'country-sources.json'
    publish_json(listing, entries)
    print(f'{PROVIDER}: {len(entries)} grids listed, {len(skipped)} off-lattice slivers skipped')
    for name in skipped:
        print(f'skipped off-lattice grid: {name}')
    return entries


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('root')
    args = parser.parse_args()
    build(args.root)


if __name__ == '__main__':
    main()
