#!/usr/bin/env python3
"""Stream the fixed ERA5 normal to global meteorology Arrow with resumable statistics."""
import argparse
import collections
from concurrent.futures import ThreadPoolExecutor
import datetime as dt
import fcntl
import hashlib
from importlib.metadata import version
import json
import os
from pathlib import Path
import shutil
import signal
import sys
import time

os.environ['OPENBLAS_NUM_THREADS'] = '1'
os.environ['OMP_NUM_THREADS'] = '1'
import numpy as np
import numba
from meteorology_io import Arco, Checkpoint, CONTRACT, VARIABLES, atomic_json, sha256, timezone_rules, write_arrow

# Anonymous GCS chunk reads are latency-bound: one step's six chunks took 3.7 s against 0.55 s of compute
# (measured 2026-09-24), so later steps are fetched while the current one is accumulated.
PREFETCH_STEPS = 8

MODEL_ROOT = Path(__file__).resolve().parents[2] / 'engine/noise-compute'
sys.path.insert(0, str(MODEL_ROOT))
from meteorology import accumulate, empty_state, prepare_hour, solar_parameters


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source-url', required=True, help='Anonymous ARCO hourly global Zarr HTTPS root')
    parser.add_argument('--retained', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--timezones', type=Path, required=True, help='meteorology-timezones JSON output')
    parser.add_argument('--threads', type=int, default=8, choices=range(1, 9))
    parser.add_argument('--stop-after', type=int, help='Stop after this many additional steps, checkpoint, do not publish')
    args = parser.parse_args()
    args.retained.mkdir(parents=True, exist_ok=True)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    lock = (args.retained / 'producer.lock').open('w')
    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    if shutil.disk_usage(args.retained).free < 16_000_000_000:
        raise RuntimeError('Need 16 GB free for two checkpoint slots and retained statistics')
    if shutil.disk_usage(args.output.parent).free < 600_000_000:
        raise RuntimeError('Need 600 MB free for atomic Arrow publication')
    files = [Path(__file__), Path(__file__).with_name('meteorology_io.py'), MODEL_ROOT / 'meteorology.py']
    zones = json.loads(args.timezones.read_text())
    rules, rules_hash = timezone_rules(zones['zones'], args.retained)
    identity = dict(timezone_rules=rules_hash, packages={name: version(name) for name in ('numpy', 'numba', 'numcodecs', 'pyarrow')}, contract=CONTRACT, source=args.source_url, timezones=sha256(args.timezones),
                    code={path.name: sha256(path) for path in files})
    checkpoint = Checkpoint(args.retained, identity)
    state, next_step, manifest_bytes = checkpoint.load()
    if state is None:
        state = empty_state(721 * 1440)
    zone_indices = np.asarray(zones['indices'], dtype=np.uint16)
    if zone_indices.shape != (721 * 1440,) or zone_indices.max() >= len(zones['zones']):
        raise ValueError('Wrong timezone grid')
    zones = rules
    first = dt.datetime(1991, 1, 1, tzinfo=dt.timezone.utc)
    end = dt.datetime(2021, 1, 1, tzinfo=dt.timezone.utc)
    total = int((end - first).total_seconds() / (3 * 3600))
    indices = np.arange(721 * 1440)
    latitudes, longitudes = 90 - (indices // 1440) * .25, (indices % 1440) * .25
    numba.set_num_threads(args.threads)
    stop = False

    def request_stop(_signum, _frame):
        nonlocal stop
        stop = True

    signal.signal(signal.SIGTERM, request_stop)
    signal.signal(signal.SIGINT, request_stop)
    started = time.monotonic()
    start_step = next_step
    checkpoint_step = next_step
    manifest_path = args.retained / 'chunks.jsonl'
    with manifest_path.open('a+b') as manifest:
        manifest.truncate(manifest_bytes)
        manifest.seek(manifest_bytes)
        arco = Arco(args.source_url, args.retained, manifest) if next_step < total else None
        with ThreadPoolExecutor(max_workers=PREFETCH_STEPS * len(VARIABLES)) as executor:
            prefetched = collections.deque()
            next_fetch = next_step
            while next_step < total and not stop:
                while next_fetch < total and len(prefetched) < PREFETCH_STEPS:
                    prefetched.append(arco.submit_hour(first + dt.timedelta(hours=3 * next_fetch), executor))
                    next_fetch += 1
                timestamp = first + dt.timedelta(hours=3 * next_step)
                fetch_started = time.monotonic()
                raw = arco.collect_hour(prefetched.popleft())
                fetch_seconds = time.monotonic() - fetch_started
                compute_started = time.monotonic()
                hours = np.array([timestamp.astimezone(zone).hour for zone in zones])
                zone_periods = np.where((hours >= 7) & (hours < 19), 0, np.where((hours >= 19) & (hours < 23), 1, 2)).astype(np.uint8)
                periods, daylight = prepare_hour(raw, zone_periods, zone_indices, latitudes, longitudes,
                                                 *solar_parameters(timestamp.timestamp()))
                if not np.isfinite(raw).all() or np.any(raw[2] <= 0) or np.any(raw[5] <= 0):
                    raise ValueError(f'Invalid physical input at {timestamp}')
                accumulate(raw, periods, daylight, **state)
                next_step += 1
                elapsed = time.monotonic() - started
                rate = (next_step - start_step) / elapsed
                print(json.dumps(dict(step=next_step, total=total, utc=timestamp.isoformat(),
                    fetch_seconds=fetch_seconds, compute_seconds=time.monotonic()-compute_started,
                    steps_per_second=rate, eta_hours=(total-next_step)/rate/3600)), flush=True)
                if args.stop_after and next_step - start_step >= args.stop_after:
                    stop = True
                if next_step % 56 == 0 or stop or next_step == total:
                    checkpoint.save(state, next_step, manifest)
                    checkpoint_step = next_step
            for requests in prefetched:
                for request in requests:
                    request.cancel()
        if next_step > checkpoint_step:
            checkpoint.save(state, next_step, manifest)
        if next_step != total:
            return
        digest = write_arrow(args.output, state, indices, dict(source_identity=args.source_url,
            source_metadata_sha256=sha256(args.retained / 'arco-metadata.json'),
            chunks_sha256=sha256(manifest_path), timezones_sha256=identity['timezones'],
            timezone_rules_sha256=rules_hash,
            producer_sha256=hashlib.sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest(),
            complete='true'))
        atomic_json(args.retained / 'meteorology.provenance.json', dict(identity=identity,
            sha256=digest, bytes=args.output.stat().st_size, steps=total,
            source_metadata_sha256=sha256(args.retained / 'arco-metadata.json'),
            chunks_sha256=sha256(manifest_path), licence='CC-BY-4.0',
            licence_url='https://creativecommons.org/licenses/by/4.0/',
            attribution='Contains modified Copernicus Climate Change Service information; ARCO-ERA5',
            completed_utc=dt.datetime.now(dt.timezone.utc).isoformat()))


if __name__ == '__main__':
    main()
