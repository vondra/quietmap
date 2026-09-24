#!/usr/bin/env python3
"""Build one complete world from frozen caches; never download sources, repaint or promote."""

import argparse
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
from dataclasses import dataclass, replace
from contextlib import ExitStack
from datetime import datetime, timedelta, timezone
import fcntl
import json
import os
from pathlib import Path
import shutil
import sys
import subprocess
import threading
import time
import tomllib

REPO = Path(__file__).resolve().parents[1]

# Pinning and producers must load the same Python/GDAL runtime and native search path.
if __name__ == '__main__':
    runtime_environment = dict(os.environ, PYTHONHASHSEED='0', PYTHONDONTWRITEBYTECODE='1')
    runtime_overrides = ('LD_LIBRARY_PATH', 'PYTHONPATH', 'PYTHONHOME')
    for key in runtime_overrides:
        runtime_environment.pop(key, None)
    if (Path(sys.prefix).resolve() != (REPO / '.venv').resolve()
            or any(key in os.environ for key in runtime_overrides)
            or os.environ.get('PYTHONHASHSEED') != '0'):
        python = str(REPO / '.venv/bin/python')
        os.execve(python, [python, '-B', str(Path(__file__).resolve()), *sys.argv[1:]], runtime_environment)
    sys.dont_write_bytecode = True

from world_build_inputs import (
    attach_rasters, audit_world, canonical_input, height_inputs,
    pin_digest, pin_inputs, raster_inputs, verify_inputs, verify_prepared_raster_links,
)

from aircraft_preflight import preflight_aircraft_sources

from world_build_state import (
    STATE_NAME, PIN_NAME, producer_command, record_steps, resume_steps, scope_unit, step_identity, write_state,
)


@dataclass(frozen=True)
class Step:
    name: str
    dependencies: tuple[str, ...]
    argv: tuple[str, ...]
    slots: int = 1
    environment: tuple[tuple[str, str], ...] = ()


def source_paths(config):
    required = {'planet', 'rasters', 'enrichment', 'boundaries', 'city_boundaries',
                'overture', 'ghsl', 'regional_heights', 'aircraft_primary', 'aircraft_secondary', 'ships',
                'ships_gfw'}
    if set(config['sources']) != required:
        raise ValueError(f'sources must be exactly {sorted(required)}')
    return {name: canonical_input(path) for name, path in config['sources'].items()}


def validate_osm_storage(paths, sources):
    for source in sources:
        source = Path(source).resolve()
        for target in paths:
            if source == target or source in target.parents or target in source.parents:
                raise ValueError(f'OSM cache/spill overlaps frozen source: {target} / {source}')


def build_plan(config, output, scratch):
    sources = source_paths(config)
    settings = config['build']
    as_of = datetime.strptime(settings['as_of_date'], '%Y%m%d')
    anchor = datetime.strptime(settings['aircraft_anchor'], '%Y-%m')
    if as_of.strftime('%Y%m%d') != settings['as_of_date'] or anchor.strftime('%Y-%m') != settings['aircraft_anchor']:
        raise ValueError('as_of_date must be YYYYMMDD and aircraft_anchor must be YYYY-MM')
    if anchor - timedelta(days=1) > as_of:
        raise ValueError('aircraft exposure year (ends the day before the anchor) ends after the source as-of date')
    storage = []
    for key, default in (('osm_node_cache', scratch / 'osm/osm_nodes.cache'),
                         ('osm_spill_dir', output / 'osm-spill')):
        value = settings.get(key, str(default))
        if not isinstance(value, str) or not value.strip():
            raise ValueError(f'{key} must be a nonempty path string')
        path = Path(value).resolve()
        if 'pre2609' in Path(value).parts or 'pre2609' in path.parts:
            raise ValueError(f'forbidden retired OSM scratch: {value}')
        if path == REPO or path in REPO.parents or REPO in path.parents:
            raise ValueError(f'{key} overlaps product checkout: {path}')
        storage.append(path)
    validate_osm_storage(storage, sources.values())
    node_cache, spill_dir = storage
    year = output / 'prepared' / str(as_of.year)
    python = str(REPO / '.venv/bin/python')
    tsx = str(REPO / 'pipeline/node_modules/.bin/tsx')
    scripts = REPO / 'scripts'
    chain = [tsx, str(REPO / 'pipeline/chain/run.ts'), '--scope', 'world',
             '--prepared-dir', str(year), '--enrichment-dir', str(sources['enrichment']),
             '--boundaries', str(sources['boundaries']), '--boundaries-dir', str(sources['city_boundaries']),
             '--as-of-date', settings['as_of_date'], '--gtfs-cache-dir', str(output / 'gtfs-cache'),
             '--jobs', str(settings['threads'])]
    layer = lambda name, dependencies: Step(name, dependencies, tuple(chain + ['--layer', name]))
    steps = [
        Step('osm', (), ('bash', str(scripts / 'osm-extract.sh')), 3,
             (('PBF_FILE', str(sources['planet'])), ('OUTPUT_DIR', str(year)),
              ('NODE_CACHE', str(node_cache)), ('SPILL_DIR', str(spill_dir)))),
        Step('square-country-city', ('osm',), (python, str(scripts / 'square-country-city/build_square_country_city.py'),
             '--prepared-dir', str(year), '--boundaries', str(sources['boundaries']),
             '--jobs', str(settings['threads'])), 2),
        layer('buildings', ('osm',)),
        Step('structures', ('buildings',), (python, str(scripts / 'structures/build-structures.py'),
             '--prepared-dir', str(year), '--overture-parquet', str(sources['overture']),
             '--ghsl', str(sources['ghsl']), '--regional', str(sources['regional_heights']),
             '--census-log', str(output / 'structures.jsonl'), '--jobs', str(settings['threads'])), 2),
        Step('structures-finalize', ('structures',), (str(REPO / 'engine/target/release/structures-finalize'), str(year))),
        layer('railways', ('square-country-city',)),
        Step('railways-finalize', ('railways',), (str(REPO / 'engine/target/release/railways-finalize'), str(year))),
        layer('industrial', ('square-country-city',)),
        layer('roads', ('square-country-city', 'structures')),
        Step('ships', ('osm',), (python, str(scripts / 'ships/build_ships.py'),
             '--prepared-dir', str(year), '--emodnet-dir', str(sources['ships']), '--gfw-dir', str(sources['ships_gfw']))),
        Step('roads-finalize', ('roads',), (str(REPO / 'engine/target/release/roads-finalize'), str(year))),
        Step('aircraft', ('osm',), ('bash', str(scripts / 'run-aircraft-extract.sh')), 2,
             (('AIRCRAFT_ANCHOR', settings['aircraft_anchor']),
              ('ADSB_CACHE', str(sources['aircraft_primary'])),
              ('SECONDARY_ADSB_CACHE', str(sources['aircraft_secondary'])),
              ('PREPARED_YEAR_DIR', str(year)), ('PREPARED_DIR', str(year)),
              ('WORK_DIR', str(scratch / 'aircraft')), ('LOG_DIR', str(output / 'aircraft-logs')),
              ('MEMMAX', ''), ('MAX_THREADS', str(settings['threads'])))),
    ]
    return year, steps


def producer_environment(threads):
    inherited = {key: os.environ[key] for key in ('PATH', 'HOME', 'USER', 'LOGNAME', 'XDG_RUNTIME_DIR',
                 'DBUS_SESSION_BUS_ADDRESS', 'CARGO_HOME', 'RUSTUP_HOME',
                 'GDAL_DATA', 'PROJ_DATA', 'PROJ_LIB') if key in os.environ}
    # A host-sized GDAL cache per spawned worker can exceed the whole producer cgroup.
    # 256 MiB retains 1024 GHSL 256x256 float32 blocks and leaves memory for vector geometry.
    return dict(inherited, LC_ALL='C', TZ='UTC', PYTHONHASHSEED='0', PROJ_NETWORK='OFF',
                GDAL_CACHEMAX='256',
                OMP_NUM_THREADS='1', OPENBLAS_NUM_THREADS='1', RAYON_NUM_THREADS=str(threads),
                QM_ROAD_WORKERS=str(threads),
                PYTHONDONTWRITEBYTECODE='1')


def give_remaining_memory(step, settings):
    budget = settings['memory_gib'] << 30
    result = subprocess.run(['systemctl', '--user', 'set-property', '--runtime',
                             scope_unit(step), f'MemoryMax={budget}'], capture_output=True, text=True)
    if result.returncode:
        # The producer can finish between its sibling and this scope update.
        if subprocess.run(['systemctl', '--user', '--quiet', 'is-active', scope_unit(step)]).returncode == 0:
            print(json.dumps({'step': step.name, 'status': 'memory-expansion-failed', 'error': result.stderr.strip()}), flush=True)
        return
    print(json.dumps({'step': step.name, 'status': 'memory-expanded', 'memory_bytes': budget}), flush=True)


def run_plan(steps, execute, completed=(), *, expand_memory=None):
    pending = {step.name: step for step in steps if step.name not in completed}
    completed, running = set(completed), {}
    failure = None
    # Expand only when no other producer can start before the remaining one finishes.
    with ThreadPoolExecutor(max_workers=4) as pool:
        while pending or running:
            free = 4 - sum(step.slots for step in running.values())
            if failure is None:
                ready = [step for step in pending.values() if set(step.dependencies) <= completed]
                if not running and len(ready) == 1:
                    step = ready[0]
                    pending[step.name] = replace(step, slots=4)
                for name, step in list(pending.items()):
                    if set(step.dependencies) <= completed and step.slots <= free:
                        running[pool.submit(execute, step)] = step
                        del pending[name]
                        free -= step.slots
            if not running:
                if failure:
                    raise failure
                raise ValueError(f'unsatisfied world dependencies: {sorted(pending)}')
            if (expand_memory is not None and len(running) == 1
                    and not any(set(step.dependencies) <= completed for step in pending.values())):
                remaining = next(iter(running.values()))
                if remaining.slots < 4:
                    expand_memory(remaining)
            done, _ = wait(running, return_when=FIRST_COMPLETED)
            for future in done:
                step = running.pop(future)
                try:
                    future.result()
                    completed.add(step.name)
                except Exception as error:
                    failure = failure or error
            if failure and not running:
                raise failure
    return completed


def require_structures_final(steps, environment):
    """Rerun the idempotent structures-finalize step: anything it writes (a z14 re-batch or an index) means a structures.arrow changed after the step."""
    step = next(step for step in steps if step.name == 'structures-finalize')
    rerun = subprocess.run(step.argv, cwd=REPO, env=dict(environment, **dict(step.environment)),
                           stdout=subprocess.PIPE, stdin=subprocess.DEVNULL, check=True, text=True)
    receipt = json.loads(rerun.stdout.splitlines()[-1])
    if receipt['blocked'] or receipt['written']:
        raise ValueError(f"{receipt['blocked']} structures.arrow re-batched and {receipt['written']} structures.qoix "
                         f'rewritten by the {step.name} rerun: a structures.arrow changed after the step; all work retained')


def code_inputs():
    names = subprocess.check_output(['git', 'ls-files', '-z', '--cached', '--others',
        '--exclude-standard', '--', 'engine', 'pipeline', 'scripts', 'rust-toolchain.toml'], cwd=REPO).decode().split('\0')
    return [REPO / name for name in names if name and (REPO / name).is_file()]



def runtime_inputs():
    executables = []
    for command in ('node', 'bash', 'cargo', 'rustc', 'systemd-run'):
        executable = shutil.which(command)
        if executable is None:
            raise ValueError(f'required world-build runtime is unavailable: {command}')
        executables.append(canonical_input(executable))
    python = REPO / '.venv/bin/python'
    standard_library = subprocess.check_output([str(python), '-c',
        'import sysconfig; print(sysconfig.get_path("stdlib"))'], text=True).strip()
    rust_sysroot = subprocess.check_output(['rustc', '--print', 'sysroot'], cwd=REPO, text=True).strip()
    return [REPO / '.venv', REPO / 'pipeline/node_modules', canonical_input(standard_library),
            canonical_input(rust_sysroot), *executables]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--scratch', type=Path, required=True)
    parser.add_argument('--plan', action='store_true')
    parser.add_argument('--resume-plan', action='store_true',
                        help='report retained/rebuilt steps and exact review identities without changing build state')
    parser.add_argument('--resume-review', type=Path,
                        help='JSON with previous/current pin hashes, reuse steps, reason; optional osm_scope: [roads, railways], aircraft_from_stage: stage2c, roads_from_step/railways_from_step: chain step')
    args = parser.parse_args()
    config = tomllib.loads(args.config.read_text())
    required = {'as_of_date', 'aircraft_anchor', 'memory_gib', 'threads'}
    optional = {'osm_node_cache', 'osm_spill_dir'}
    if (set(config) != {'build', 'sources'} or not required <= set(config['build'])
            or set(config['build']) - required - optional):
        raise ValueError('build requires as_of_date, aircraft_anchor, memory_gib and threads')
    settings = config['build']
    if any(type(settings[key]) is not int or settings[key] < 1 for key in ('memory_gib', 'threads')):
        raise ValueError('memory_gib and threads must be positive integers')
    output, scratch = args.output.resolve(), args.scratch.resolve()
    sources = source_paths(config)
    for target in (output, scratch):
        if target == REPO or target in REPO.parents or REPO in target.parents:
            raise ValueError('output cannot contain the product checkout')
        for source in sources.values():
            if source == target or source in target.parents or target in source.parents:
                raise ValueError(f'output/scratch overlaps source: {target} / {source}')
    if output == scratch or output in scratch.parents or scratch in output.parents:
        raise ValueError('output and scratch must be separate directories')
    year, steps = build_plan(config, output, scratch)
    for step in steps:
        print(json.dumps(dict(step=step.name, after=step.dependencies, argv=step.argv,
                              environment=dict(step.environment), memory_bytes=(settings['memory_gib'] << 30) * step.slots // 4)), flush=True)
    if args.plan:
        return
    # A failed build of this configuration resumes in place (`resume_steps`); never
    # adopt unrelated or partly written generations as successful upstream work.
    resuming = (output / STATE_NAME).is_file()
    if (args.resume_plan or args.resume_review) and not resuming:
        raise ValueError('resume options require an existing build')
    review = json.loads(args.resume_review.read_text()) if args.resume_review else None
    if not resuming:
        for target in (output, scratch):
            if target.exists() and any(target.iterdir()):
                raise ValueError(f'requires a fresh directory; prior output retained: {target}')
        # The multi-terabyte aircraft caches are the one input family whose sampling
        # window cannot be judged from the pin; admit the exact days read-only before
        # pinning, directory creation or any producer can start.
        admitted = preflight_aircraft_sources(
            sources['aircraft_primary'], sources['aircraft_secondary'], settings['aircraft_anchor'])
        print(json.dumps({'step': 'aircraft-preflight', **admitted}), flush=True)
    output.mkdir(parents=True, exist_ok=True)
    scratch.mkdir(parents=True, exist_ok=True)
    environment = producer_environment(settings['threads'])
    environment['PATH'] = str(REPO / '.venv/bin') + os.pathsep + environment.get('PATH', os.defpath)
    # Pin-time GDAL and every producer must resolve the same local datum files.
    os.environ.clear()
    os.environ.update(environment)
    os.chdir(REPO)
    with ExitStack() as locks:
        for target in (output, scratch):
            lock = locks.enter_context((target / '.world-build.lock').open('a'))
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        completed = set()
        pin_path = output / PIN_NAME
        receipts_lock = threading.Lock()
        def frozen_roots():
            ordinary = [path for name, path in sources.items() if name not in ('rasters', 'ghsl', 'regional_heights')]
            return [*ordinary, *raster_inputs(sources['rasters']), *height_inputs(sources['ghsl']),
                    *height_inputs(sources['regional_heights'])]
        def current_roots():
            return [*frozen_roots(), *code_inputs(), *runtime_inputs(),
                    *(canonical_input(environment[key]) for key in ('GDAL_DATA', 'PROJ_DATA', 'PROJ_LIB')
                      if key in environment)]
        roots = current_roots()
        osm_environment = dict(next(step for step in steps if step.name == 'osm').environment)
        validate_osm_storage([Path(osm_environment[key]) for key in ('NODE_CACHE', 'SPILL_DIR')], roots)
        if resuming:
            completed = resume_steps(output, config, steps, roots, frozen_roots(),
                                     review=review, dry_run=args.resume_plan)
            if args.resume_plan:
                return
        else:
            pin_inputs(pin_path, roots)
            write_state(output, config, 'running')
        input_pin_sha256 = pin_digest(pin_path)
        try:
            attach_rasters(sources['rasters'], year)
            # Build before parallel producers so their incremental builds share no changing code.
            subprocess.run(['cargo', 'build', '--release', '--manifest-path', str(REPO / 'engine/Cargo.toml'),
                            '--bin', 'osm-extract', '--bin', 'aircraft-extract', '--bin', 'structures-finalize',
                            '--bin', 'railways-finalize', '--bin', 'roads-finalize'],
                           cwd=REPO, env=environment, check=True)
            def execute(step):
                command = producer_command(step, settings)
                started = time.time()
                receipt = dict(name=step.name, started=started,
                               **step_identity(step, settings, input_pin_sha256))
                with receipts_lock:
                    record_steps(output, [dict(receipt, exit=None)])
                print(json.dumps({'step': step.name, 'status': 'running'}), flush=True)
                with (output / f'{step.name}.log').open('ab') as log:
                    log.write(f'=== attempt {datetime.now(timezone.utc).isoformat()} ===\n'.encode())
                    log.flush()
                    result = subprocess.run(command, cwd=REPO, env=dict(environment, **dict(step.environment)),
                                            stdout=log, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL)
                with receipts_lock:
                    record_steps(output, [{
                        **receipt,
                        'seconds': time.time() - started, 'exit': result.returncode,
                    }])
                print(json.dumps({'step': step.name, 'exit': result.returncode}), flush=True)
                if result.returncode:
                    raise RuntimeError(f'{step.name} failed; inspect {output / (step.name + ".log")}; all work retained')
            run_plan(steps, execute, completed, expand_memory=lambda step: give_remaining_memory(step, settings))
            require_structures_final(steps, environment)
            counts = audit_world(year, settings['threads'])
            verify_prepared_raster_links(sources['rasters'], year)
            verify_inputs(pin_path, current_roots())
            write_state(output, config, 'complete', rows=counts)
            print(json.dumps(dict(status='complete', prepared=str(year), rows=counts)), flush=True)
        except BaseException:
            write_state(output, config, 'failed')
            raise


if __name__ == '__main__':
    main()
