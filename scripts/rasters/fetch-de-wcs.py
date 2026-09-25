"""Fetch official 1 m terrain through INSPIRE/WCS windows reduced to 5 m in-flight.

Nordrhein-Westfalen, Baden-Württemberg and Brandenburg/Berlin expose their
current DGM1 mosaic as a WCS 2.0.1 coverage. Each 10 x 10 km window is requested
at 5 m (SCALESIZE), so the server does the stream reduction and only the
retained 5 m grids plus a checksum manifest of the raw responses stay on disk.
"""
import argparse
import math
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import threading
import time

from osgeo import gdal
from dgm_reduce import (DERIVED_NODATA, append_journal, assert_crs, clean_remnants,
                        derive_provenance, derived_complete, digest, load_journal,
                        manifest_entry, normalize_grid, publish_country_sources,
                        write_manifest)
from terrain_io import fetch, source_budget

gdal.UseExceptions()
gdal.SetConfigOption('GDAL_PAM_ENABLED', 'NO')

PROVIDERS = {
    'de-nw-dgm1': dict(
        endpoint='https://www.wcs.nrw.de/geobasis/wcs_nw_dgm', coverage='nw_dgm', axes=('x', 'y'),
        scale_axes=('x', 'y'),
        subsetting_crs='http://www.opengis.net/def/crs/EPSG/0/25832',
        extent=(278000.0, 5560000.0, 536000.0, 5828000.0), epsg=25832,
        licence='Datenlizenz Deutschland - Zero - Version 2.0',
        licence_url='https://www.govdata.de/dl-de/zero-2-0', group='DE-NW-DGM1',
        epoch='mixed ALS epochs served as the current WCS mosaic'),
    'de-bw-dgm1': dict(
        endpoint='https://owsproxy.lgl-bw.de/owsproxy/wcs/WCS_INSP_BW_Hoehe_Coverage_DGM1',
        coverage='EL.ElevationGridCoverage', axes=('E', 'N'), scale_axes=('X', 'Y'),
        subsetting_crs='http://www.opengis.net/def/crs/EPSG/0/25832',
        extent=(387999.5, 5263999.5, 611000.5, 5520000.5), epsg=25832,
        licence='Datenlizenz Deutschland - Namensnennung - Version 2.0',
        licence_url='https://www.govdata.de/dl-de/by-2-0', group='DE-BW-DGM1',
        epoch='mixed ALS epochs served as the current WCS mosaic'),
    'de-bb-dgm1': dict(
        endpoint='https://isk.geobasis-bb.de/ows/dgm_wcs', coverage='bb_dgm', axes=('x', 'y'),
        scale_axes=('x', 'y'),
        subsetting_crs='http://www.opengis.net/def/crs/EPSG/0/25833',
        extent=(228152.0, 5690412.0, 493382.0, 5939023.0), epsg=25833,
        licence='Datenlizenz Deutschland - Namensnennung - Version 2.0',
        licence_url='https://www.govdata.de/dl-de/by-2-0', group='DE-BB-DGM1',
        epoch='mixed ALS epochs served as the current WCS mosaic, Berlin included'),
}

WINDOW_METRES = 10000
RESOLUTION_METRES = 5
VERTICAL_EPSG = 7837


def windows(extent):
    west, south, east, north = extent
    columns = math.ceil((east - west) / WINDOW_METRES)
    rows = math.ceil((north - south) / WINDOW_METRES)
    for row in range(rows):
        for column in range(columns):
            w = west + column * WINDOW_METRES
            n = north - row * WINDOW_METRES
            yield w, max(n - WINDOW_METRES, south), min(w + WINDOW_METRES, east), n


def coverage_url(provider, west, south, east, north):
    config = PROVIDERS[provider]
    axe, axn = config['axes']
    # Scaling addresses grid axes, which Baden-Württemberg names X/Y, not E/N.
    sce, scn = config['scale_axes']
    size_x = int(round((east - west) / RESOLUTION_METRES))
    size_y = int(round((north - south) / RESOLUTION_METRES))
    query = (f'SERVICE=WCS&VERSION=2.0.1&REQUEST=GetCoverage&COVERAGEID={config["coverage"]}'
             f'&FORMAT=image/tiff&SUBSETTINGCRS={config["subsetting_crs"]}'
             f'&SUBSET={axe}({west:.1f},{east:.1f})&SUBSET={axn}({south:.1f},{north:.1f})'
             f'&SCALESIZE={sce}({size_x}),{scn}({size_y})')
    return f'{config["endpoint"]}?{query}'


def fetch_window(root, provider, box, delay, state, done):
    config = PROVIDERS[provider]
    west, south, east, north = box
    raw_name = f'wcs_{west:.0f}_{south:.0f}.raw.tif'
    out_name = f'wcs_{west:.0f}_{south:.0f}.tif'
    out_path = Path(root) / provider / out_name
    if out_name in done and derived_complete(out_path):
        return None
    clean_remnants(out_path)
    url = coverage_url(provider, west, south, east, north)
    if url in state['empty']:
        return None
    with state['lock']:
        wait = delay - (time.monotonic() - state['last'])
        if wait > 0:
            time.sleep(wait)
        state['last'] = time.monotonic()
    fetch(root, provider, raw_name, url, licence=config['licence'],
          licence_url=config['licence_url'], terms_checked_utc='2026-09-25')
    raw_path = Path(root) / provider / raw_name
    dataset = gdal.Open(str(raw_path))
    band = dataset.GetRasterBand(1)
    nodata = band.GetNoDataValue()
    size = (dataset.RasterXSize, dataset.RasterYSize)
    try:
        minimum, maximum, _, _ = band.ComputeStatistics(False)
    except RuntimeError:
        minimum = maximum = nodata
    dataset = None
    if nodata is not None and minimum == maximum == nodata:
        # A fully empty window adds no coverage; the manifest still records it.
        entry = dict(url=url, raw_bytes=raw_path.stat().st_size, raw_sha256=digest(raw_path),
                     derived=None, derived_sha256=None, derived_bytes=0,
                     method='empty WCS window, nothing retained')
        raw_path.unlink()
        Path(str(raw_path) + '.provenance.json').unlink()
        append_journal(state['journal'], entry, state['lock'])
        return entry
    if min(size) < 2:
        raise ValueError(f'{raw_path}: degenerate WCS window')
    # Brandenburg answers carry no projection; the documented request CRS is assigned.
    normalize_grid(raw_path, out_path, assign_epsg=config['epsg'])
    assert_crs(out_path, config['epsg'], VERTICAL_EPSG)
    derive_provenance(raw_path, out_path,
                      f'Normalised {RESOLUTION_METRES} m WCS window from the verified DGM1 response')
    entry = manifest_entry(url, raw_path, out_path,
                           f'Server-side {RESOLUTION_METRES} m reduction of official DGM1')
    raw_path.unlink()
    Path(str(raw_path) + '.provenance.json').unlink()
    append_journal(state['journal'], entry, state['lock'])
    return entry


def fetch_all(root, provider, workers=2, delay=1.0, limit=None, box=None):
    config = PROVIDERS[provider]
    journal = Path(root) / provider / 'stream-manifest.journal.jsonl'
    journal.parent.mkdir(parents=True, exist_ok=True)
    entries, done = load_journal(journal)
    empty = set(e['url'] for e in entries if not e.get('derived'))
    boxes = [box] if box else list(windows(config['extent']))[:limit]
    state = {'lock': threading.Lock(), 'last': 0.0, 'journal': journal, 'empty': empty}
    with ThreadPoolExecutor(max_workers=workers) as pool:
        futures = [pool.submit(fetch_window, root, provider, item, delay, state, done)
                   for item in boxes]
        for future in futures:
            result = future.result()
            if result is not None:
                entries.append(result)
    manifest = Path(root) / provider / 'stream-manifest.json'
    try:
        write_manifest(manifest, entries)
    except ValueError:
        manifest.unlink()
        write_manifest(manifest, entries)
    value = publish_country_sources(root, provider, entries, config['epsg'],
                                    VERTICAL_EPSG, config['group'], config['epoch'])
    with source_budget(Path(root)) as available:
        print(f'{provider}: {len(value)} windows retained, {available / 1e9:.1f} GB budget left')
    return entries


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('root')
    parser.add_argument('provider', choices=sorted(PROVIDERS))
    parser.add_argument('--workers', type=int, default=2)
    parser.add_argument('--delay', type=float, default=1.0)
    parser.add_argument('--limit', type=int, default=None)
    parser.add_argument('--box', type=float, nargs=4, default=None,
                        metavar=('WEST', 'SOUTH', 'EAST', 'NORTH'))
    args = parser.parse_args()
    fetch_all(args.root, args.provider, workers=args.workers, delay=args.delay,
              limit=args.limit, box=args.box)


if __name__ == '__main__':
    main()
