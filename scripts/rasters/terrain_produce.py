#!/usr/bin/env python3
"""Area-average reviewed terrain/canopy sources onto canonical Rust z9 node windows."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import time

import numpy as np
from osgeo import gdal, osr
from canopy_average import canopy_average
from terrain_io import digest, provenance, publish_bytes, publish_json

gdal.UseExceptions()
osr.UseExceptions()


def raster_window(binary, x, y):
    return json.loads(subprocess.check_output([str(binary), 'window', str(x), str(y)], text=True))


def bounds(window):
    d = window['nodes_per_degree']
    north, west = window['north_node'], window['west_node']
    return [(west - .5) / d, (north - window['rows'] + .5) / d,
            (west + window['columns'] - .5) / d, (north + .5) / d]


def read_average(source, window, kernel='average'):
    dataset = source['path'] if isinstance(source['path'], gdal.Dataset) else gdal.Open(str(source['path']))
    if dataset.RasterCount != 1:
        raise ValueError('terrain sources must contain one band')
    # GDAL ReadAsArray/warp consumes raw values; never apply GEDTM's erroneous scale tag.
    result = gdal.Warp('', dataset, format='MEM', outputType=gdal.GDT_Float64,
                       srcSRS=source['horizontal_crs'], dstSRS='EPSG:4326',
                       outputBounds=bounds(window), width=window['columns'], height=window['rows'],
                       srcNodata=source.get('nodata', dataset.GetRasterBand(1).GetNoDataValue()),
                       dstNodata=float('nan'), resampleAlg=kernel, errorThreshold=0,
                       overviewLevel='NONE', warpOptions=['NUM_THREADS=1'], options=['-novshift'])
    return result.ReadAsArray()


def datum_transform(vertical_crs):
    source, target = osr.SpatialReference(), osr.SpatialReference()
    source.SetFromUserInput('EPSG:4326+' + str(vertical_crs))
    target.SetFromUserInput('EPSG:4326+3855')
    for crs in (source, target):
        crs.SetAxisMappingStrategy(osr.OAMS_TRADITIONAL_GIS_ORDER)
    options = osr.CoordinateTransformationOptions()
    options.SetBallparkAllowed(False)
    options.SetOnlyBest(True)
    return osr.CreateCoordinateTransformation(source, target, options)


def convert_datum(values, window, vertical_crs):
    """Apply the geoid shift at each target node after area averaging (never a constant offset)."""
    if vertical_crs == 3855:
        return values
    transform = datum_transform(vertical_crs)
    density = window['nodes_per_degree']
    columns = (window['west_node'] + np.arange(window['columns'])) / density
    for row in range(len(values)):
        latitude = (window['north_node'] - row) / density
        valid = np.flatnonzero(np.isfinite(values[row]))
        if not len(valid):
            continue
        points = [(float(columns[c]), latitude, float(values[row, c])) for c in valid]
        converted = np.asarray(transform.TransformPoints(points))
        if not np.isfinite(converted).all():
            raise ValueError('national datum conversion failed; missing grids are not a zero shift')
        values[row, valid] = converted[:, 2]
    return values


def encode(values, channel, window):
    finite = np.isfinite(values)
    if channel == 'dem':
        offset, factor, missing = (window[k] for k in ('dem_offset_m', 'dem_codes_per_metre', 'dem_missing'))
        codes = (values[finite] - offset) * factor
        if np.any((codes < 0) | (codes > missing - 1)):
            raise ValueError('DEM outside encoding range')
        result = np.full(values.shape, missing, dtype='<u2')
        result[finite] = np.floor(codes + .5).astype('<u2')
    else:
        if np.any((values[finite] < 0) | (values[finite] > 250)):
            raise ValueError('canopy outside 0..250 metres')
        result = np.full(values.shape, 255, dtype='u1')
        result[finite] = np.floor(values[finite] + .5).astype('u1')
    return result


def assemble(sources, window, channel, kernel='average'):
    values = np.full((window['rows'], window['columns']), np.nan)
    owner = np.full(values.shape, -1, dtype=np.int16)
    # Manifest order is low to high precedence; last valid national DTM overrides fallback.
    for index, source in enumerate(sources):
        sampled = (canopy_average(source, window, read_average) if channel == 'canopy'
                   else read_average(source, window, kernel))
        if channel == 'dem':
            sampled = convert_datum(sampled, window, source['vertical_crs'])
        valid = np.isfinite(sampled)
        values[valid], owner[valid] = sampled[valid], index
    return values, owner


def produce(manifest, output, binary, reserve_bytes, kernel='average'):
    manifest_bytes = Path(manifest).read_bytes()
    plan = json.loads(manifest_bytes)
    channel = plan['channel']
    if channel not in ('dem', 'canopy'):
        raise ValueError('expected dem or canopy')
    sources = plan['sources']
    source_records = [provenance(source['path']) for source in sources]
    grids = [provenance(path) for path in plan.get('geoid_grids', [])]
    osr.SetPROJSearchPaths(osr.GetPROJSearchPaths() + sorted({str(Path(p).parent) for p in plan.get('geoid_grids', [])}))
    identity_bytes = manifest_bytes + kernel.encode() + json.dumps(source_records + grids, sort_keys=True).encode()
    producer_files = ('terrain_produce.py', 'canopy_average.py', 'terrain_io.py')
    producer_hashes = {name: digest(Path(__file__).with_name(name)) for name in producer_files}
    producer_sha256 = hashlib.sha256(json.dumps(producer_hashes, sort_keys=True).encode()).hexdigest()
    identity_bytes += (producer_sha256 + digest(binary)).encode()
    output = Path(output)
    output.mkdir(parents=True, exist_ok=True)
    for x, y in plan['squares']:
        started = time.monotonic()
        window = raster_window(binary, x, y)
        size = window['rows'] * window['columns'] * (2 if channel == 'dem' else 1)
        if shutil.disk_usage(output).free < reserve_bytes + 2 * size:
            raise ValueError('raster release would consume the prepared/scratch space reserve')
        extension = 'u16le' if channel == 'dem' else 'u8'
        path = output / 'z9' / str(x) / str(y) / f'{channel}.{extension}'
        sidecar = Path(str(path) + '.provenance.json')
        identity = hashlib.sha256(identity_bytes).hexdigest()
        if sidecar.exists():
            previous = json.loads(sidecar.read_text())
            if previous['plan_sha256'] != identity or digest(path) != previous['sha256']:
                raise ValueError(f'published square differs from resumed plan: {path}')
            print(json.dumps({'x': x, 'y': y, 'resumed': True}), flush=True)
            continue
        values, owner = assemble(sources, window, channel, kernel)
        missing = int(np.count_nonzero(~np.isfinite(values)))
        if missing:
            raise ValueError(f'z9/{x}/{y}: {missing} unavailable {channel} nodes; refusing publication')
        encoded = encode(values, channel, window).tobytes()
        record = dict(channel=channel, window=window, kernel=kernel, datum='EGM2008' if channel == 'dem' else 'above bare earth',
                      plan_sha256=identity, sha256=hashlib.sha256(encoded).hexdigest(), bytes=len(encoded),
                      sources=[dict(**r, acquisition_epoch=s['epoch'], coverage_fraction=float(np.mean(owner == i)))
                               for i, (s, r) in enumerate(zip(sources, source_records))],
                      missing_nodes=missing, seam_statistics=None, geoid_grids=grids, producer_sha256=producer_sha256,
                      elapsed_seconds=time.monotonic() - started)
        publish_bytes(path, encoded)
        publish_json(sidecar, record)
        print(json.dumps({'x': x, 'y': y, 'bytes': len(encoded), 'seconds': record['elapsed_seconds']}), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('manifest', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--raster-repack', type=Path, required=True)
    parser.add_argument('--reserve-bytes', type=int, required=True, help='Space reserved for prepared, rollback and scratch')
    args = parser.parse_args()
    if args.reserve_bytes < 0:
        parser.error('reserve must be nonnegative')
    osr.SetPROJEnableNetwork(False)
    gdal.SetCacheMax(256 * 1024 * 1024)
    produce(args.manifest, args.output, args.raster_repack, args.reserve_bytes)


if __name__ == '__main__':
    main()
