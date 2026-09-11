#!/usr/bin/env python3
"""Build one complete world from frozen caches; never download sources, repaint or promote."""

import argparse
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
from dataclasses import dataclass
from contextlib import ExitStack
from datetime import datetime
import fcntl
import json
import os
from pathlib import Path
import sqlite3
import shutil
import sys
import subprocess
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

from prepared_manifest import file_identity, write_manifest
from world_build_inputs import attach_rasters, audit_world, canonical_input, pin_inputs, verify_inputs, raster_inputs, height_inputs, verify_prepared_raster_links


@dataclass(frozen=True)
class Step:
    name: str
    dependencies: tuple[str, ...]
    argv: tuple[str, ...]
    slots: int = 1
    environment: tuple[tuple[str, str], ...] = ()


def source_paths(config):
    required = {'planet', 'rasters', 'enrichment', 'boundaries', 'city_boundaries',
                'overture', 'ghsl', 'regional_heights', 'airline', 'general_aviation'}
    if set(config['sources']) != required:
        raise ValueError(f'sources must be exactly {sorted(required)}')
    return {name: canonical_input(path) for name, path in config['sources'].items()}


def build_plan(config, output, scratch):
    sources = source_paths(config)
    settings = config['build']
    as_of = datetime.strptime(settings['as_of_date'], '%Y%m%d')
    anchor = datetime.strptime(settings['aircraft_anchor'], '%Y-%m')
    if as_of.strftime('%Y%m%d') != settings['as_of_date'] or anchor.strftime('%Y-%m') != settings['aircraft_anchor']:
        raise ValueError('as_of_date must be YYYYMMDD and aircraft_anchor must be YYYY-MM')
    if anchor > as_of.replace(day=1):
        raise ValueError('aircraft anchor is after the source as-of date')
    year = output / 'prepared' / str(as_of.year)
    python = str(REPO / '.venv/bin/python')
    tsx = str(REPO / 'pipeline/node_modules/.bin/tsx')
    scripts = REPO / 'scripts'
    chain = [tsx, str(REPO / 'pipeline/chain/run.ts'), '--scope', 'world',
             '--prepared-dir', str(year), '--enrichment-dir', str(sources['enrichment']),
             '--boundaries', str(sources['boundaries']), '--boundaries-dir', str(sources['city_boundaries']),
             '--as-of-date', settings['as_of_date'], '--gtfs-cache-dir', str(output / 'gtfs-cache')]
    layer = lambda name, dependencies: Step(name, dependencies, tuple(chain + ['--layer', name]))
    steps = [
        Step('osm', (), ('bash', str(scripts / 'osm-extract.sh')), 3,
             (('PBF_FILE', str(sources['planet'])), ('OUTPUT_DIR', str(year)),
              ('SCRATCH_ROOT', str(scratch / 'osm')))),
        Step('square-country-city', ('osm',), (python, str(scripts / 'square-country-city/build_square_country_city.py'),
             '--prepared-dir', str(year), '--boundaries', str(sources['boundaries']),
             '--jobs', str(settings['threads'])), 3),
        layer('buildings', ('osm',)),
        Step('structures', ('buildings',), (python, str(scripts / 'structures/build-structures.py'),
             '--prepared-dir', str(year), '--overture-parquet', str(sources['overture']),
             '--ghsl', str(sources['ghsl']), '--regional', str(sources['regional_heights']),
             '--census-log', str(output / 'structures.jsonl')), 3),
        Step('structures-finalize', ('structures',), (str(REPO / 'engine/target/release/structures-finalize'), str(year))),
        layer('railways', ('square-country-city',)),
        layer('industrial', ('square-country-city',)),
        layer('roads', ('square-country-city', 'structures')),
        Step('aircraft', ('osm',), ('bash', str(scripts / 'run-aircraft-extract.sh')), 3,
             (('HYBRID', '1'), ('AIRLINE_FEED', 'adsbexchange'), ('AIRCRAFT_ANCHOR', settings['aircraft_anchor']),
              ('AIRLINE_CACHE', str(sources['airline'])), ('GA_CACHE', str(sources['general_aviation'])),
              ('PREPARED_YEAR_DIR', str(year)), ('PREPARED_DIR', str(year)),
              ('WORK_DIR', str(scratch / 'aircraft')), ('LOG_DIR', str(output / 'aircraft-logs')),
              ('MEMMAX', ''), ('MAX_THREADS', str(settings['threads'])))),
    ]
    return year, steps


def producer_environment(threads):
    inherited = {key: os.environ[key] for key in ('PATH', 'HOME', 'USER', 'LOGNAME', 'XDG_RUNTIME_DIR',
                 'DBUS_SESSION_BUS_ADDRESS', 'CARGO_HOME', 'RUSTUP_HOME',
                 'GDAL_DATA', 'PROJ_DATA', 'PROJ_LIB') if key in os.environ}
    return dict(inherited, LC_ALL='C', TZ='UTC', PYTHONHASHSEED='0', PROJ_NETWORK='OFF',
                OMP_NUM_THREADS='1', OPENBLAS_NUM_THREADS='1', RAYON_NUM_THREADS=str(threads),
                PYTHONDONTWRITEBYTECODE='1')


def run_plan(steps, execute, completed=()):
    pending = {step.name: step for step in steps if step.name not in completed}
    completed, running = set(completed), {}
    failure = None
    # Three memory shares go to OSM/structures/aircraft/square-country-city (20 resolver
    # processes need ~25 GiB), one to a layer writer.
    # Admission plus matching cgroup limits bounds their combined working sets.
    with ThreadPoolExecutor(max_workers=4) as pool:
        while pending or running:
            free = 4 - sum(step.slots for step in running.values())
            if failure is None:
                for name, step in list(pending.items()):
                    if set(step.dependencies) <= completed and step.slots <= free:
                        running[pool.submit(execute, step)] = step
                        del pending[name]
                        free -= step.slots
            if not running:
                if failure:
                    raise failure
                raise ValueError(f'unsatisfied world dependencies: {sorted(pending)}')
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


def resume_steps(database, config, code_files):
    """Steps of a failed build in `database` that exited 0 and stay done; the rest rerun. Refuses another config or a complete build; reports code files that changed since the pin, then drops the pin for a fresh one."""
    (pinned_config, status), = database.execute('SELECT config, status FROM build')
    if pinned_config != json.dumps(config, sort_keys=True) or status == 'complete':
        raise ValueError(f'cannot resume a {status} build of another configuration; prior output retained')
    completed = {name for name, in database.execute('SELECT name FROM steps WHERE exit = 0')}
    # A resume interrupted while pinning left no or an empty inputs table (the pin commits
    # once, at its end): the previous pin is unknown.
    changed = 'unpinned'
    if database.execute("SELECT 1 FROM sqlite_master WHERE name = 'inputs'").fetchone():
        pinned = {path: identity for path, *identity in database.execute(
            'SELECT path,device,inode,bytes,mtime_ns,ctime_ns FROM inputs')}
        if pinned:
            changed = sorted(str(path) for path in code_files
                             if pinned.get(str(path)) != list(file_identity(path)))
        database.execute('DROP TABLE inputs')
    database.execute('DELETE FROM steps WHERE exit IS NULL OR exit != 0')
    database.execute("UPDATE build SET status='running'")
    database.commit()
    print(json.dumps({'resume': sorted(completed), 'code_changed': changed}), flush=True)
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
    args = parser.parse_args()
    config = tomllib.loads(args.config.read_text())
    if set(config) != {'build', 'sources'} or set(config['build']) != {
            'as_of_date', 'aircraft_anchor', 'memory_gib', 'threads'}:
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
    resuming = (output / 'build.sqlite').is_file()
    if not resuming:
        for target in (output, scratch):
            if target.exists() and any(target.iterdir()):
                raise ValueError(f'requires a fresh directory; prior output retained: {target}')
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
        database = sqlite3.connect(output / 'build.sqlite')
        completed = set()
        if resuming:
            completed = resume_steps(database, config, code_inputs())
        else:
            database.execute('CREATE TABLE build(config TEXT NOT NULL, status TEXT NOT NULL)')
            database.execute('INSERT INTO build VALUES(?,?)', (json.dumps(config, sort_keys=True), 'running'))
            database.execute('CREATE TABLE steps(name TEXT PRIMARY KEY, command TEXT NOT NULL, '
                             'started REAL NOT NULL, seconds REAL, exit INTEGER)')
            database.commit()
        def current_roots():
            ordinary = [path for name, path in sources.items() if name not in ('rasters', 'ghsl', 'regional_heights')]
            return [*ordinary, *raster_inputs(sources['rasters']), *height_inputs(sources['ghsl']),
                    *height_inputs(sources['regional_heights']), *code_inputs(), *runtime_inputs(),
                    *(canonical_input(environment[key]) for key in ('GDAL_DATA', 'PROJ_DATA', 'PROJ_LIB')
                      if key in environment)]
        try:
            roots = current_roots()
            pin_inputs(database, roots)
            attach_rasters(sources['rasters'], year)
            # Build before parallel producers so their incremental builds share no changing code.
            subprocess.run(['cargo', 'build', '--release', '--manifest-path', str(REPO / 'engine/Cargo.toml'),
                            '--bin', 'osm-extract', '--bin', 'aircraft-extract', '--bin', 'structures-finalize'],
                           cwd=REPO, env=environment, check=True)
            def execute(step):
                budget = (settings['memory_gib'] << 30) * step.slots // 4
                command = ['systemd-run', '--user', '--scope', '--quiet', '-p', f'MemoryMax={budget}',
                           '-p', 'MemorySwapMax=0', *step.argv]
                started = time.time()
                with sqlite3.connect(output / 'build.sqlite', timeout=60) as record:
                    record.execute('INSERT INTO steps(name,command,started) VALUES(?,?,?)',
                                   (step.name, json.dumps(command), started))
                print(json.dumps({'step': step.name, 'status': 'running'}), flush=True)
                with (output / f'{step.name}.log').open('ab') as log:
                    result = subprocess.run(command, cwd=REPO, env=dict(environment, **dict(step.environment)),
                                            stdout=log, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL)
                with sqlite3.connect(output / 'build.sqlite', timeout=60) as record:
                    record.execute('UPDATE steps SET seconds=?,exit=? WHERE name=?',
                                   (time.time() - started, result.returncode, step.name))
                print(json.dumps({'step': step.name, 'exit': result.returncode}), flush=True)
                if result.returncode:
                    raise RuntimeError(f'{step.name} failed; inspect {output / (step.name + ".log")}; all work retained')
            run_plan(steps, execute, completed)
            require_structures_final(steps, environment)
            counts = audit_world(year)
            verify_prepared_raster_links(sources['rasters'], year)
            verify_inputs(database, current_roots())
            manifest = write_manifest(year, year / 'inputs.sqlite')
            database.execute('CREATE TABLE output(manifest_sha256 TEXT NOT NULL, counts TEXT NOT NULL)')
            database.execute('INSERT INTO output VALUES(?,?)',
                             (manifest['sha256'], json.dumps(counts, sort_keys=True)))
            database.execute("UPDATE build SET status='complete'")
            database.commit()
            print(json.dumps(dict(status='complete', prepared=str(year), manifest=manifest, rows=counts)), flush=True)
        except BaseException:
            database.execute("UPDATE build SET status='failed'")
            database.commit()
            raise
        finally:
            database.close()


if __name__ == '__main__':
    main()
