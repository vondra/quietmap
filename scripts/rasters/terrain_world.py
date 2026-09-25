#!/usr/bin/env python3
"""Launch a resumable terrain/canopy world only from complete, externally reviewed coverage."""
import argparse
import gzip
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
from osgeo import gdal, osr
from terrain_io import digest, publish_json, MAX_DOWNLOAD_BYTES
from terrain_produce import produce


def world_plan(coverage_path, expected_sha256):
    if digest(coverage_path) != expected_sha256:
        raise ValueError('world coverage differs from the reviewed manifest')
    with gzip.open(coverage_path, 'rt') as stream:
        plan = json.load(stream)
    squares = plan['squares']
    coordinates = {(r['x'], r['y']) for r in squares}
    if len(squares) != 512**2 or coordinates != {(x, y) for x in range(512) for y in range(512)}:
        raise ValueError('world coverage must classify every z9 square exactly once')
    if any(r['status'] not in ('land', 'ocean') for r in squares):
        raise ValueError('world coverage contains unassessed squares')
    unresolved = [r for r in squares if r.get('terrain_source_status') == 'unresolved_polar_land']
    if unresolved:
        raise ValueError(f'{len(unresolved)} polar land squares lack a reviewed bare-earth source')
    return plan


def preflight(args):
    coverage = world_plan(args.coverage, args.coverage_sha256)
    if not args.source_root.is_dir() or not args.raster_repack.is_file():
        raise ValueError('source root and canonical raster binary must exist')
    used = sum(p.stat().st_size for p in args.source_root.rglob('*') if p.is_file())
    if used > MAX_DOWNLOAD_BYTES:
        raise ValueError('combined terrain/canopy retained-source budget exceeded')
    land = [(r['x'], r['y']) for r in coverage['squares'] if r['status'] == 'land']
    size = sum(r['rows'] * r['columns'] * 3 for r in coverage['squares'] if r['status'] == 'land')
    # Resuming is conservative: count only correctly sized completed channel payloads as reclaimed budget.
    completed = 0
    for r in coverage['squares']:
        if r['status'] != 'land':
            continue
        for name, width in (('dem.u16le', 2), ('canopy.u8', 1)):
            p = args.output / 'z9' / str(r['x']) / str(r['y']) / name
            if p.exists() and p.stat().st_size == r['rows'] * r['columns'] * width:
                completed += p.stat().st_size
    parent = args.output if args.output.exists() else args.output.parent
    if shutil.disk_usage(parent).free < size - completed + args.reserve_bytes:
        raise ValueError('world output plus prepared/scratch reserve does not fit')
    for channel, path in (('dem', args.dem), ('canopy', args.canopy)):
        source_plan = json.loads(path.read_text())
        if source_plan['channel'] != channel or set(map(tuple, source_plan['squares'])) != set(land):
            raise ValueError(f'{channel} plan must cover every land square in the reviewed world')
        dependencies = list(source_plan.get('geoid_grids', []))
        dependencies.extend(s[key] for s in source_plan['sources']
                            for key in ('path', 'dtm_path', 'vegetation_mask_path') if key in s)
        if any(not Path(p).resolve().is_relative_to(args.source_root.resolve()) for p in dependencies):
            raise ValueError('world input lies outside the combined source-budget root')
    return coverage


def run(args):
    coverage = preflight(args)
    # One worker avoids races between neighbouring seam checks and gives a hard thread bound.
    for channel, path in (('dem', args.dem), ('canopy', args.canopy)):
        produce(path, args.output, args.raster_repack, args.reserve_bytes,
                dict(squares=coverage['squares'], sha256=args.coverage_sha256))
    publish_json(args.output / 'terrain-world-complete.json', dict(coverage_sha256=args.coverage_sha256,
                 dem_plan_sha256=digest(args.dem), canopy_plan_sha256=digest(args.canopy)))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('check', 'launch', 'run'))
    for name in ('coverage', 'dem', 'canopy', 'source-root', 'output', 'raster-repack'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--coverage-sha256', required=True, help='Digest recorded by independent manifest review')
    parser.add_argument('--reserve-bytes', type=int, required=True)
    parser.add_argument('--unit', default='terrain-world')
    args = parser.parse_args()
    osr.SetPROJEnableNetwork(False)
    gdal.SetCacheMax(256 * 1024 * 1024)
    if args.reserve_bytes < 0 or not re.fullmatch(r'[A-Za-z0-9_-]+', args.unit):
        parser.error('invalid reserve or systemd unit name')
    if args.action == 'run':
        run(args)
    else:
        preflight(args)
        if args.action == 'launch':
            command = [sys.executable, str(Path(__file__).resolve()), 'run']
            for key in ('coverage', 'dem', 'canopy', 'source_root', 'output', 'raster_repack', 'coverage_sha256', 'reserve_bytes'):
                command.extend(['--' + key.replace('_', '-'), str(getattr(args, key))])
            subprocess.run(['systemd-run', '--user', '--collect', '--same-dir', '--unit=' + args.unit, '-p', 'Nice=10',
                            '-p', 'CPUQuota=800%', '-p', 'MemoryMax=8G',
                            '-E', 'OPENBLAS_NUM_THREADS=1', '-E', 'OMP_NUM_THREADS=1',
                            '-E', 'GDAL_NUM_THREADS=1', '-E', 'RAYON_NUM_THREADS=1', *command], check=True)
        print('world preflight passed')


if __name__ == '__main__':
    main()
