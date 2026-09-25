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
from terrain_seams import (expanded, feather, artificial_steps, require_seam_gate,
                           verify_shared_nodes, QUANTIZATION_STEP_BUDGET_M)

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


def datum_transform(vertical_crs, area_of_interest=None):
    source, target = osr.SpatialReference(), osr.SpatialReference()
    source.SetFromUserInput('EPSG:4326+' + str(vertical_crs))
    target.SetFromUserInput('EPSG:4326+3855')
    for crs in (source, target):
        crs.SetAxisMappingStrategy(osr.OAMS_TRADITIONAL_GIS_ORDER)
    options = osr.CoordinateTransformationOptions()
    options.SetBallparkAllowed(False)
    options.SetOnlyBest(True)
    # A reviewed regional anchor selects one grid operation across rounded EPSG area edges.
    if area_of_interest is not None:
        options.SetAreaOfInterest(*area_of_interest)
    return osr.CreateCoordinateTransformation(source, target, options)


def convert_datum(values, window, vertical_crs, area_of_interest=None):
    """Apply the geoid shift at each target node after area averaging (never a constant offset)."""
    if vertical_crs == 3855:
        return values
    transform = datum_transform(vertical_crs, area_of_interest)
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


def grouped_sources(sources):
    """Mosaic each region before averaging, so source-file edges are not coverage edges."""
    # Source CRS comes from reviewed provider metadata, not redundant GeoTIFF parameter copies.
    gdal.SetConfigOption('GTIFF_SRS_SOURCE', 'EPSG')
    groups = {}
    for index, source in enumerate(sources):
        key = source.get('group', str(index))
        if key in groups and list(groups)[-1] != key:
            raise ValueError('source group members must be contiguous in precedence order')
        groups.setdefault(key, []).append(source)
    result = []
    for key, members in groups.items():
        first = members[0]
        if len(members) > 1 and any('dtm_path' in s for s in members):
            raise ValueError('derive classified canopy before mosaicking multiple DSM tiles')
        for source in members:
            for field in ('horizontal_crs', 'vertical_crs', 'epoch', 'role', 'nodata', 'datum_area_of_interest'):
                if source.get(field) != first.get(field):
                    raise ValueError(f'{key}: mixed {field} in a source mosaic')
        native = gdal.Open(str(first['path']))
        transform = native.GetGeoTransform()
        if transform[2] or transform[4] or transform[1] <= 0 or transform[5] >= 0:
            raise ValueError('source groups require a north-up native pixel grid')
        for member in members:
            dataset = gdal.Open(str(member['path']))
            grid = dataset.GetGeoTransform()
            offset = [(grid[0] - transform[0]) / transform[1], (grid[3] - transform[3]) / transform[5]]
            if (not np.allclose([grid[1], grid[5]], [transform[1], transform[5]], rtol=1e-12, atol=0)
                    or grid[2] or grid[4] or not np.allclose(offset, np.rint(offset), atol=1e-6, rtol=0)):
                raise ValueError('source group members must share their native pixel grid')
            if dataset.RasterCount != 1 or dataset.GetRasterBand(1).DataType != native.GetRasterBand(1).DataType:
                raise ValueError('source group members must have one band with the same data type')
        missing = 255 if native.GetRasterBand(1).DataType == gdal.GDT_Byte else float('nan')
        mosaic = (gdal.BuildVRT('', [str(s['path']) for s in members], VRTNodata=missing, srcNodata=first.get('nodata'), strict=True,
                               resolution='user', xRes=transform[1], yRes=-transform[5]) if len(members) > 1
                  else native)
        if mosaic is None:
            raise ValueError(f'cannot build source mosaic: {key}')
        result.append(dict(first, path=mosaic, group=key,
                           **({'nodata': missing} if len(members) > 1 else {})))
    return result


def assemble_with_statistics(sources, window, channel, kernel='average', halo=128):
    national = channel == 'dem' and any(s.get('role') == 'national' for s in sources)
    padding = halo + 2 if national else 0
    work = expanded(window, padding)
    values = np.full((work['rows'], work['columns']), np.nan)
    owner = np.full(values.shape, -1, dtype=np.int16)
    statistics = dict(halo_nodes=halo if national else 0, groups=[],
                      maximum_artificial_step_bound_m=QUANTIZATION_STEP_BUDGET_M)
    for index, source in enumerate(sources):
        sampled = (canopy_average(source, work, read_average) if channel == 'canopy'
                   else read_average(source, work, kernel))
        if channel == 'dem':
            sampled = convert_datum(sampled, work, source['vertical_crs'], source.get('datum_area_of_interest'))
        if channel == 'dem' and source.get('role') == 'national':
            values, weight, difference = feather(values, sampled, halo)
            group = dict(group=source['group'], **artificial_steps(weight, difference, padding))
            statistics['groups'].append(group)
            statistics['maximum_artificial_step_bound_m'] += group['maximum_selection_step_m']
            valid = weight > 0
        else:
            valid = np.isfinite(sampled)
            values[valid] = sampled[valid]
        owner[valid] = index
    if padding:
        values = values[padding:-padding, padding:-padding]
        owner = owner[padding:-padding, padding:-padding]
    return values, owner, statistics


def produce(manifest, output, binary, reserve_bytes, ocean_coverage=None):
    kernel = 'average'
    manifest_bytes = Path(manifest).read_bytes()
    plan = json.loads(manifest_bytes)
    channel = plan['channel']
    if channel not in ('dem', 'canopy'):
        raise ValueError('expected dem or canopy')
    sources = plan['sources']
    if channel == 'dem' and any(s.get('role') not in ('fallback', 'national') for s in sources):
        raise ValueError('DEM sources must declare fallback or national role')
    if any(s.get('role') == 'national' and not s.get('group') for s in sources):
        raise ValueError('national source files must declare their coverage group')
    roles = [s['role'] for s in sources] if channel == 'dem' else []
    fallback_groups = {s.get('group', str(i)) for i, s in enumerate(sources) if s.get('role') == 'fallback'}
    if channel == 'dem' and (len(fallback_groups) != 1 or not roles or roles != sorted(roles)):
        raise ValueError('DEM requires one initial fallback mosaic followed by national coverage')
    source_records = [provenance(source['path']) for source in sources]
    auxiliary_records = [{key: provenance(s[key]) for key in ('dtm_path', 'vegetation_mask_path') if key in s}
                         for s in sources]
    mosaics = grouped_sources(sources)
    grids = [provenance(path) for path in plan.get('geoid_grids', [])]
    osr.SetPROJSearchPaths(osr.GetPROJSearchPaths() + sorted({str(Path(p).parent) for p in plan.get('geoid_grids', [])}))
    identity_bytes = manifest_bytes + kernel.encode() + json.dumps(source_records + auxiliary_records + grids, sort_keys=True).encode()
    producer_files = ('terrain_produce.py', 'canopy_average.py', 'terrain_io.py', 'terrain_seams.py')
    producer_hashes = {name: digest(Path(__file__).with_name(name)) for name in producer_files}
    producer_sha256 = hashlib.sha256(json.dumps(producer_hashes, sort_keys=True).encode()).hexdigest()
    identity_bytes += (producer_sha256 + digest(binary)).encode()
    identity_bytes += json.dumps(ocean_coverage['sha256'] if ocean_coverage else None).encode()
    output = Path(output)
    output.mkdir(parents=True, exist_ok=True)
    identity = hashlib.sha256(identity_bytes).hexdigest()
    input_record = dict(sources=source_records, source_specs=sources, auxiliary=auxiliary_records, geoid_grids=grids)
    input_bytes = (json.dumps(input_record, sort_keys=True) + '\n').encode()
    input_hash = hashlib.sha256(input_bytes).hexdigest()
    input_name = f'{channel}-inputs-{input_hash}.json'
    publish_bytes(output / input_name, input_bytes)
    # Ocean receipts must exist before coastal land, so the first build checks those shared nodes.
    for square in ocean_coverage['squares'] if ocean_coverage else []:
        if square['status'] != 'ocean':
            continue
        extension = 'u16le' if channel == 'dem' else 'u8'
        target = output / 'z9' / str(square['x']) / str(square['y']) / f'{channel}.{extension}'
        publish_bytes(target, b'')
        publish_json(str(target) + '.provenance.json', dict(channel=channel, bytes=0,
                     sha256=hashlib.sha256(b'').hexdigest(), coverage_verified_ocean=True,
                     plan_sha256=identity, coverage_manifest_sha256=ocean_coverage['sha256']))
    for x, y in plan['squares']:
        started = time.monotonic()
        window = raster_window(binary, x, y)
        size = window['rows'] * window['columns'] * (2 if channel == 'dem' else 1)
        if shutil.disk_usage(output).free < reserve_bytes + 2 * size:
            raise ValueError('raster release would consume the prepared/scratch space reserve')
        extension = 'u16le' if channel == 'dem' else 'u8'
        path = output / 'z9' / str(x) / str(y) / f'{channel}.{extension}'
        sidecar = Path(str(path) + '.provenance.json')
        if sidecar.exists():
            previous = json.loads(sidecar.read_text())
            if previous['plan_sha256'] != identity or digest(path) != previous['sha256']:
                raise ValueError(f'published square differs from resumed plan: {path}')
            print(json.dumps({'x': x, 'y': y, 'resumed': True}), flush=True)
            continue
        values, owner, seams = assemble_with_statistics(mosaics, window, channel, kernel, plan.get('feather_halo_nodes', 128))
        if channel == 'dem':
            require_seam_gate(seams)
        missing = int(np.count_nonzero(~np.isfinite(values)))
        if missing:
            raise ValueError(f'z9/{x}/{y}: {missing} unavailable {channel} nodes; refusing publication')
        codes = encode(values, channel, window)
        if channel == 'dem':
            seams.update(verify_shared_nodes(path, codes, window, binary, identity, raster_window))
        encoded = codes.tobytes()
        record = dict(channel=channel, window=window, kernel=kernel, datum='EGM2008' if channel == 'dem' else 'above bare earth',
                      plan_sha256=identity, sha256=hashlib.sha256(encoded).hexdigest(), bytes=len(encoded),
                      source_manifest=dict(path=input_name, sha256=input_hash),
                      sources=[dict(group=s['group'], acquisition_epoch=s['epoch'],
                                    coverage_fraction=float(np.mean(owner == i))) for i, s in enumerate(mosaics)],
                      missing_nodes=missing, seam_statistics=seams, geoid_grids=grids, producer_sha256=producer_sha256,
                      elapsed_seconds=time.monotonic() - started)
        publish_bytes(path, encoded)
        publish_json(sidecar, record)
        print(json.dumps({'x': x, 'y': y, 'bytes': len(encoded), 'seconds': record['elapsed_seconds']}), flush=True)
    return identity


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
