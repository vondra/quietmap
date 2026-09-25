"""Build a terrain-produce manifest from provider country-sources and a square list.

Fallback sources come first, then one contiguous group per national
provider. Squares default to the z9 squares intersecting a lon/lat box;
--land-only keeps the squares whose every node the fallback covers, since
mixed coastal squares need the water mask that is not part of this task.
"""
import argparse
import json
from pathlib import Path

from terrain_io import publish_json
from terrain_produce import bounds, raster_window

from osgeo import gdal

gdal.UseExceptions()


def squares_in_box(binary, west, south, east, north):
    """z9 squares intersecting the box; the Rust binary owns the mapping."""
    xs = range(int((west + 180) / 360 * 512), int((east + 180) / 360 * 512) + 1)
    found = []
    for x in xs:
        for y in range(512):
            window = raster_window(binary, x, y)
            w, s, e, n = bounds(window)
            if e > west and w < east and n > south and s < north:
                found.append([x, y])
    return sorted(found)


def fallback_covers(fallback_path, binary, square, samples=200):
    """One coarse warp decides whether the fallback covers every node."""
    window = raster_window(binary, *square)
    probe = gdal.Open(str(fallback_path))
    nodata = probe.GetRasterBand(1).GetNoDataValue()
    probe = None
    result = gdal.Warp('', str(fallback_path), format='MEM', outputType=gdal.GDT_Float32,
                       outputBounds=bounds(window), width=samples, height=samples,
                       srcNodata=nodata, dstNodata=float('nan'),
                       resampleAlg=gdal.GRA_NearestNeighbour, errorThreshold=0)
    values = result.ReadAsArray()
    return bool((values == values).all())


def load_sources(root, providers):
    sources = []
    for provider in providers:
        listing = Path(root) / provider / 'country-sources.json'
        entries = json.loads(listing.read_text())
        if not entries:
            raise ValueError(f'{provider}: no sources listed; refusing a silent gap')
        for entry in entries:
            if not Path(entry['path']).is_file():
                raise ValueError(f"missing source file: {entry['path']}")
            if not Path(entry['path'] + '.provenance.json').is_file():
                raise ValueError(f"missing source provenance: {entry['path']}")
        sources.extend(entries)
    groups = [s.get('group', '') for s in sources]
    if len(set(groups)) != len(providers):
        raise ValueError('each provider must contribute exactly one source group')
    if groups != sorted(groups, key=providers.index):
        raise ValueError('source groups are not contiguous in precedence order')
    return sources


def build(args):
    binary = Path(args.raster_repack)
    fallback = load_sources(args.root, [args.fallback])
    if any(s.get('role') != 'fallback' for s in fallback):
        raise ValueError('fallback provider must declare the fallback role')
    national = load_sources(args.root, args.providers)
    if any(s.get('role') != 'national' for s in national):
        raise ValueError('national providers must declare the national role')
    squares = squares_in_box(binary, *args.bbox)
    print(f'{len(squares)} squares intersect the box')
    if args.land_only:
        squares = [q for q in squares
                   if fallback_covers(fallback[0]['path'], binary, q)]
        print(f'{len(squares)} squares fully covered by the fallback')
    manifest = dict(channel='dem', sources=fallback + national, squares=squares,
                    geoid_grids=[str(Path(args.root) / g) for g in args.grids],
                    feather_halo_nodes=args.halo)
    for grid in manifest['geoid_grids']:
        if not Path(grid).is_file():
            raise ValueError(f'missing geoid grid: {grid}')
    publish_json(args.out, manifest)
    print(f'wrote {args.out}: {len(manifest["sources"])} sources, '
          f'{len(squares)} squares, halo {args.halo}')
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', required=True)
    parser.add_argument('--fallback', required=True)
    parser.add_argument('--providers', nargs='+', required=True)
    parser.add_argument('--grids', nargs='+', required=True)
    parser.add_argument('--bbox', type=float, nargs=4, required=True,
                        metavar=('WEST', 'SOUTH', 'EAST', 'NORTH'))
    parser.add_argument('--land-only', action='store_true')
    parser.add_argument('--halo', type=int, default=256)
    parser.add_argument('--raster-repack', required=True)
    parser.add_argument('--out', required=True)
    args = parser.parse_args()
    build(args)


if __name__ == '__main__':
    main()
