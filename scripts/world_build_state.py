"""Atomic world-build receipts and safe reuse of completed producer steps."""

from datetime import datetime, timezone
import json
import os
from pathlib import Path
import subprocess
import tempfile

from world_build_inputs import repin_inputs

STATE_NAME = 'build.json'
STEPS_NAME = 'steps.jsonl'
PIN_NAME = 'input-identities.jsonl'


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
    state.update(config=json.dumps(config, sort_keys=True), status=status, **fields)
    write_atomic(path, json.dumps(state, sort_keys=True) + '\n')


def record_step(output, receipt):
    path = output / STEPS_NAME
    previous = path.read_text() if path.exists() else ''
    write_atomic(path, previous + json.dumps(receipt, sort_keys=True) + '\n')


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


def completed_steps(path, steps, settings):
    latest = {}
    if path.is_file():
        for line in path.read_text().splitlines():
            if line.strip():
                row = json.loads(line)
                latest[row['name']] = row
    completed = set()
    for step in steps:
        row = latest.get(step.name)
        if row and row.get('exit') == 0 and step.name != 'structures-finalize':
            if producer_arguments(row['command']) == producer_arguments(producer_command(step, settings)):
                completed.add(step.name)
    # Rebuilding an upstream table invalidates every completed consumer of it.
    while True:
        stale = {step.name for step in steps if step.name in completed
                 and not set(step.dependencies) <= completed}
        if not stale:
            return completed
        completed -= stale


def resume_steps(output, config, steps, roots, repo):
    state = json.loads((output / STATE_NAME).read_text())
    def data_configuration(value):
        return {**value, 'build': {key: item for key, item in value['build'].items()
                                  if key not in ('threads', 'memory_gib')}}
    if data_configuration(json.loads(state['config'])) != data_configuration(config):
        raise ValueError('cannot resume another configuration; prior output retained')
    if state['status'] == 'complete':
        raise ValueError('cannot resume a complete build; prior output retained')
    live = [step.name for step in steps if subprocess.run(
        ['systemctl', '--user', '--quiet', 'is-active', scope_unit(step)], check=False).returncode == 0]
    if live:
        raise ValueError(f'cannot resume while producers are alive: {live}')
    completed = completed_steps(output / STEPS_NAME, steps, config['build'])
    changed = repin_inputs(output / PIN_NAME, roots, repo)
    report = {'at': datetime.now(timezone.utc).isoformat(timespec='seconds'),
              'resume': sorted(completed), 'code_changed': changed}
    write_state(output, config, 'running', resumes=[*state.get('resumes', []), report])
    print(json.dumps(report), flush=True)
    return completed
