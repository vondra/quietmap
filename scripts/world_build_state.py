"""Atomic world-build receipts and safe reuse of completed producer steps."""

from dataclasses import replace
from datetime import datetime, timezone
from functools import cache
import json
import os
from pathlib import Path
import subprocess
import tempfile

from world_build_inputs import pin_digest, repin_inputs

STATE_NAME = 'build.json'
STEPS_NAME = 'steps.jsonl'
PIN_NAME = 'input-identities.jsonl'
REPOSITORY = Path(__file__).resolve().parents[1]


@cache
def product_code_identity(repository=REPOSITORY):
    """The checked-out product commit and whether the working tree differs from it.

    Read once per process: the input pin covers every producer code file, so a later producer
    edit fails the build instead of mislabelling it.
    """
    def git(*arguments):
        return subprocess.check_output(['git', *arguments], cwd=repository, text=True)
    return {'product_commit': git('rev-parse', 'HEAD').strip(),
            'product_dirty': bool(git('status', '--porcelain', '--untracked-files=normal').strip())}


def write_atomic(path, text):
    output = path.parent
    descriptor, temporary = tempfile.mkstemp(prefix=f'.{path.name}.', dir=output)
    try:
        with os.fdopen(descriptor, 'w', encoding='utf-8') as stream:
            stream.write(text)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        directory = os.open(output, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        Path(temporary).unlink(missing_ok=True)


def write_state(output, config, status, **fields):
    path = output / STATE_NAME
    state = json.loads(path.read_text()) if path.exists() else {}
    state.update(config=json.dumps(config, sort_keys=True), status=status, **product_code_identity(), **fields)
    state.pop('remaining', None)
    write_atomic(path, json.dumps(state, sort_keys=True) + '\n')


def record_steps(output, receipts):
    path = output / STEPS_NAME
    previous = path.read_text() if path.exists() else ''
    write_atomic(path, previous + ''.join(json.dumps(row, sort_keys=True) + '\n' for row in receipts))


def scope_unit(step):
    return f'world-build-{step.name}.scope'


def producer_command(step, settings):
    budget = (settings['memory_gib'] << 30) * step.slots // 4
    return ['systemd-run', '--user', '--scope', '--quiet', '--unit', scope_unit(step),
            '-p', f'MemoryMax={budget}', '-p', 'MemorySwapMax=0', *step.argv]


def producer_arguments(command):
    # Scope names and resource caps change scheduling, not the produced data.
    argv = iter(command[command.index('MemorySwapMax=0') + 1:])
    result = []
    for argument in argv:
        if argument == '--jobs':
            next(argv)
        else:
            result.append(argument)
    return result


def latest_receipts(path):
    latest = {}
    if path.is_file():
        for line in path.read_text().splitlines():
            if line.strip():
                row = json.loads(line)
                latest[row['name']] = row
    return latest


def data_environment(environment):
    # Admission, worker count and temporary storage placement do not change produced data.
    return {key: value for key, value in environment.items()
            if key not in ('MAX_THREADS', 'MEMMAX', 'RAYON_NUM_THREADS', 'QM_ROAD_WORKERS',
                           'SCRATCH_ROOT', 'NODE_CACHE', 'SPILL_DIR')}


def step_identity(step, settings, input_pin_sha256):
    return {'command': producer_command(step, settings),
            'environment': dict(step.environment), 'input_pin_sha256': input_pin_sha256}


def completed_steps(latest, steps, settings, input_pin_sha256, reviewed=()):
    completed = set()
    for step in steps:
        row = latest.get(step.name)
        if not row or row.get('exit') != 0 or step.name == 'structures-finalize':
            continue
        same_command = producer_arguments(row['command']) == producer_arguments(producer_command(step, settings))
        same_environment = ('environment' in row and
                            data_environment(row['environment']) == data_environment(dict(step.environment)))
        reviewed_legacy_environment = step.name in reviewed and 'environment' not in row
        if same_command and (same_environment or reviewed_legacy_environment):
            if row.get('input_pin_sha256') == input_pin_sha256 or step.name in reviewed:
                completed.add(step.name)
    # A producer rerun invalidates every consumer, including explicitly reviewed reuse.
    while True:
        stale = {step.name for step in steps if step.name in completed
                 and not set(step.dependencies) <= completed}
        if not stale:
            return completed
        completed -= stale


def interrupted_chain_step(output, name, previous, step, settings, input_pin):
    """Resume the last started substep only inside the same immutable attempt."""
    if (previous.get('exit') == 0 or previous.get('input_pin_sha256') != input_pin
            or not isinstance(previous.get('started'), (int, float))):
        return None
    def without_boundary(arguments):
        return arguments[:-2] if arguments[-2:-1] == ['--from'] else arguments
    if (without_boundary(producer_arguments(previous['command'])) !=
            without_boundary(producer_arguments(producer_command(step, settings)))
            or data_environment(previous.get('environment', {})) != data_environment(dict(step.environment))):
        return None
    path = output / f'{name}.log'
    if not path.is_file():
        return None
    boundary, current_attempt = None, False
    with path.open(errors='replace') as log:
        for line in log:
            if line.startswith('=== attempt '):
                boundary = None
                try:
                    current_attempt = datetime.fromisoformat(line.removeprefix('=== attempt ').split(' ===')[0]).timestamp() >= previous['started']
                except ValueError:
                    current_attempt = False
            elif current_attempt and line.startswith('{'):
                try:
                    row = json.loads(line)
                except ValueError:
                    continue
                if (isinstance(row, dict) and isinstance(row.get('argv'), list)
                        and isinstance(row.get('step'), str)
                        and row['step'].startswith(name + '-')):
                    boundary = row['step']
    return boundary


def resume_steps(output, config, steps, roots, frozen_roots, *, review=None, dry_run=False):
    state = json.loads((output / STATE_NAME).read_text())
    def data_configuration(value):
        return {**value, 'build': {key: item for key, item in value['build'].items()
                                  if key not in ('threads', 'memory_gib', 'osm_node_cache', 'osm_spill_dir')}}
    if data_configuration(json.loads(state['config'])) != data_configuration(config):
        raise ValueError('cannot resume another configuration; prior output retained')
    live = [step.name for step in steps if subprocess.run(
        ['systemctl', '--user', '--quiet', 'is-active', scope_unit(step)], check=False).returncode == 0]
    if live:
        raise ValueError(f'cannot resume while producers are alive: {live}')
    pin_path = output / PIN_NAME
    with repin_inputs(pin_path, roots, frozen_roots) as (candidate, changed, sources):
        previous_digest, current_digest = pin_digest(pin_path), pin_digest(candidate)
        latest = latest_receipts(output / STEPS_NAME)
        reviewed = set()
        if review is not None:
            if (not isinstance(review, dict)
                    or not {'previous_pin_sha256', 'current_pin_sha256', 'reuse', 'reason'} <= set(review)
                    or set(review) - {'previous_pin_sha256', 'current_pin_sha256', 'reuse', 'reason', 'osm_scope', 'aircraft_from_stage', 'roads_from_step', 'railways_from_step'}
                    or review['previous_pin_sha256'] != previous_digest
                    or review['current_pin_sha256'] != current_digest
                    or not isinstance(review['reason'], str) or not review['reason'].strip()
                    or not isinstance(review['reuse'], list)
                    or any(not isinstance(name, str) for name in review['reuse'])):
                raise ValueError('resume review must name the exact previous/current pins, reuse steps and a reason')
            reviewed = set(review['reuse'])
        persisted_scope = state.get('osm_scope', [])
        osm_scope = review.get('osm_scope', persisted_scope) if review is not None else persisted_scope
        if osm_scope != [] and osm_scope != ['roads', 'railways']:
            raise ValueError('osm_scope must be exactly [roads, railways]')
        if persisted_scope and osm_scope != persisted_scope:
            raise ValueError('cannot change the persisted OSM resume scope')
        original_steps = steps
        persisted_aircraft_stage = state.get('aircraft_from_stage')
        aircraft_stage = review.get('aircraft_from_stage', persisted_aircraft_stage) if review is not None else persisted_aircraft_stage
        if aircraft_stage not in (None, 'stage2b', 'stage2c') or (review is not None
                and 'aircraft_from_stage' in review and aircraft_stage is None):
            raise ValueError('aircraft_from_stage must be stage2b or stage2c')
        aircraft = latest.get('aircraft', {})
        if aircraft_stage != persisted_aircraft_stage:
            if aircraft_stage == 'stage2b':
                if (review is None or not aircraft.get('command') or 'environment' not in aircraft
                        or aircraft.get('exit') != 0 or aircraft.get('input_pin_sha256') != previous_digest):
                    raise ValueError('cruise-only replay requires a reviewed successful aircraft receipt at the previous pin')
            elif persisted_aircraft_stage:
                raise ValueError('cannot change the persisted aircraft resume stage')
            elif aircraft_stage and (not aircraft.get('command')
                    or 'environment' not in aircraft or aircraft.get('exit') == 0
                    or aircraft.get('input_pin_sha256') != previous_digest):
                raise ValueError('aircraft stage resume requires an unsuccessful aircraft attempt at the previous pin')
        if aircraft_stage:
            stage_environment = {'FROM_STAGE': aircraft_stage}
            if aircraft_stage == 'stage2b':
                stage_environment['UNTIL_STAGE'] = aircraft_stage
            steps = [replace(step, environment=tuple(dict(step.environment, **stage_environment).items()))
                     if step.name == 'aircraft' else step for step in steps]
        chain_starts = {}
        for name in ('roads', 'railways'):
            key = f'{name}_from_step'
            persisted = state.get(key)
            boundary = review.get(key, persisted) if review is not None else persisted
            if review is None and state['status'] != 'complete':
                step = next((step for step in steps if step.name == name), None)
                if step is not None:
                    boundary = interrupted_chain_step(output, name, latest.get(name, {}),
                        step, config['build'], current_digest) or boundary
            if boundary is not None or (review is not None and key in review):
                if not isinstance(boundary, str) or not boundary.strip():
                    raise ValueError(f'{key} must be a nonempty chain step name')
                previous = latest.get(name, {})
                reviewed_rebuild = state['status'] == 'complete' and review is not None
                if boundary != persisted and (not previous.get('command')
                        or 'environment' not in previous
                        or (previous.get('exit') == 0 and not reviewed_rebuild)
                        or previous.get('input_pin_sha256') != previous_digest):
                    raise ValueError(f'{name} step resume requires an unsuccessful {name} attempt at the previous pin')
                steps = [replace(step, argv=(step.argv[:-2] if step.argv[-2:-1] == ('--from',) else step.argv) + ('--from', boundary))
                         if step.name == name and step.argv[-2:] != ('--from', boundary) else step
                         for step in steps]
            chain_starts[name] = boundary
        chain_fields = {f'{name}_from_step': boundary for name, boundary in chain_starts.items()}
        if osm_scope:
            osm = latest.get('osm', {})
            if not persisted_scope and (osm.get('exit') != 0
                    or osm.get('environment', {}).get('QM_OSM_ONLY')):
                raise ValueError('transport resume requires a previously successful full osm extraction')
            if 'osm' in reviewed and osm.get('environment', {}).get('QM_OSM_ONLY') != ','.join(osm_scope):
                raise ValueError('cannot adopt full osm receipt for transport-only resume')
            steps = [replace(step, environment=tuple(dict(step.environment,
                         QM_OSM_ONLY=','.join(osm_scope)).items())) if step.name == 'osm' else step
                     for step in steps]
        completed = completed_steps(latest, steps, config['build'], current_digest, reviewed)
        if reviewed - completed:
            raise ValueError(f'cannot adopt unsuccessful, changed-command/environment or dependent steps: {sorted(reviewed - completed)}')
        invalidated = sorted(step.name for step in steps if step.name not in completed
                             and step.name != 'structures-finalize'
                             and latest.get(step.name, {}).get('exit') == 0)
        report = {'at': datetime.now(timezone.utc).isoformat(timespec='seconds'),
                  'resume': sorted(completed), 'rebuild': [step.name for step in steps if step.name not in completed],
                  'invalidated': invalidated, 'producer_inputs_changed': changed,
                  'frozen_inputs_changed': sources, 'osm_scope': osm_scope,
                  'aircraft_from_stage': aircraft_stage, **chain_fields,
                  'review': review or {'previous_pin_sha256': previous_digest,
                      'current_pin_sha256': current_digest, 'reuse': [], 'reason': '',
                      **({'osm_scope': osm_scope} if osm_scope else {}),
                      **({'aircraft_from_stage': aircraft_stage} if aircraft_stage else {}),
                      **{key: boundary for key, boundary in chain_fields.items() if boundary}}}
        print(json.dumps(report), flush=True)
        if dry_run:
            return completed
        if sources and review is None:
            raise ValueError(f'{len(sources)} frozen sources changed since the pin (examples: {sources[:3]}); '
                             'inspect --resume-plan and supply --resume-review naming the steps whose outputs do not read them')
        if state['status'] == 'complete' and (review is None or not invalidated):
            raise ValueError('complete build requires an exact resume review that rebuilds completed producer outputs')
        for name, boundary in (('aircraft', aircraft_stage), *chain_starts.items()):
            if boundary:
                step = next(step for step in steps if step.name == name)
                if not set(step.dependencies) <= completed:
                    raise ValueError(f'{name} resume requires retained upstream outputs: {step.dependencies}')
        if invalidated and review is None:
            raise ValueError('completed outputs need review before rebuilding; inspect --resume-plan and supply --resume-review JSON')
        rows = [{'name': name, 'exit': None, 'invalidated': report['at']} for name in invalidated]
        rows += [dict(latest[step.name], **step_identity(step, config['build'], current_digest),
                      adopted=report['at'], review=review)
                 for step in steps if step.name in reviewed]
        # Publish all decisions before replacing the pin. A crash can discard reuse,
        # but cannot resurrect a consumer after its producer was invalidated.
        # Persist resume boundaries first: interrupted receipt/pin publication must
        # never restart an approved partial producer from its beginning.
        write_state(output, config, 'running', osm_scope=osm_scope,
                    aircraft_from_stage=aircraft_stage, **chain_fields,
                    resumes=[*state.get('resumes', []), report])
        if rows:
            record_steps(output, rows)
        os.replace(candidate, pin_path)
        original_steps[:] = steps
        return completed
