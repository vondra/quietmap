"""Bake road built_up (0 unknown, 1 rural, 2 urban) from prepared structures_v4."""

import argparse
from concurrent.futures import ProcessPoolExecutor
from collections import Counter
import fcntl
from functools import cache
import hashlib
import json
from pathlib import Path
import sys

import pyarrow as pa

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
from prepared_arrow import rewrite_arrow_batches, segment_midpoints  # noqa: E402
from qmgrid import parse_square_name, square_name, Z9_AXIS  # noqa: E402
from building_footprints import BuildingFootprintSampler  # noqa: E402
from worker_jobs import available_memory_bytes, cpu_jobs, fit_jobs  # noqa: E402

# 16 workers filled the 20 GiB road-layer cgroup (1/4 of an 80 GiB world build).
WORKER_BYTES = (20 << 30) // 16


@cache
def classification_code_identity():
    files = (Path(__file__), Path(__file__).with_name('building_footprints.py'),
             Path(__file__).resolve().parents[1] / 'lib/qmgrid.py')
    return hashlib.sha256(b''.join(path.read_bytes() for path in files)).hexdigest()


def classification_inputs(path):
    """The owner and its halo are the complete density inputs for this road file."""
    x, y = int(path.parent.parent.name), int(path.parent.name)
    prepared = path.parents[3]
    identities = []
    for dx in (-1, 0, 1):
        for dy in (-1, 0, 1):
            if not 0 <= y + dy < Z9_AXIS:
                continue
            source = prepared / square_name((x + dx) % Z9_AXIS, y + dy) / 'structures.arrow'
            try:
                stat = source.stat()
                identities.append([str(source), stat.st_dev, stat.st_ino, stat.st_size, stat.st_mtime_ns, stat.st_ctime_ns])
            except FileNotFoundError:
                identities.append([str(source), None])
    return hashlib.sha256(json.dumps([classification_code_identity(), identities],
        separators=(',', ':')).encode()).hexdigest().encode()


def bake_file(path, sampler):
    identity = classification_inputs(path)
    with pa.memory_map(str(path), 'r') as source:
        schema = pa.ipc.open_file(source).schema
        if (schema.metadata or {}).get(b'qm_built_up_inputs') == identity:
            return {'files_changed': 0, 'files_skipped': 1}
    counts = Counter()

    def classify_batch(batch):
        if (batch.schema.metadata or {}).get(b"grid") != b"z30":
            raise ValueError(f"{path}: expected grid z30 roads")
        latitudes, longitudes = segment_midpoints(batch)
        values = [sampler.classify(lat, lon) for lat, lon in zip(latitudes, longitudes)]
        counts.update(values)
        array = pa.array(values, type=pa.uint8())
        index = batch.schema.get_field_index("built_up")
        if index < 0:
            batch = batch.append_column(pa.field("built_up", pa.uint8(), nullable=False), array)
        else:
            if batch.column(index).type != pa.uint8() or batch.column(index).null_count:
                raise ValueError(f"{path}: built_up must be a non-null UInt8 column")
            batch = batch.set_column(index, batch.schema.field(index), array)
        return batch.replace_schema_metadata({**(batch.schema.metadata or {}), b'qm_built_up_inputs': identity})

    rows, changed = rewrite_arrow_batches(path, classify_batch)
    return {"rows": rows, "unknown": counts[0], "rural": counts[1], "urban": counts[2],
            "files_changed": int(changed)}


_WORKER_SAMPLER = None


def initialize_worker(prepared_dir):
    global _WORKER_SAMPLER
    _WORKER_SAMPLER = BuildingFootprintSampler(prepared_dir)


def bake_square(prepared_dir, name):
    sampler = _WORKER_SAMPLER or BuildingFootprintSampler(prepared_dir)
    path = Path(prepared_dir) / name / "roads.arrow"
    return name, bake_file(path, sampler)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prepared-dir", type=Path, required=True)
    parser.add_argument("--square", action="append", help="Repeat to select existing z9/x/y units")
    parser.add_argument("--workers", type=int, default=None,
                        help="Worker cap (default: all CPUs that fit memory; 1 keeps the serial path)")
    args = parser.parse_args()
    if args.workers is not None and args.workers < 1:
        parser.error("--workers must be >= 1")
    prepared = args.prepared_dir.resolve(strict=True)
    names = sorted(set(args.square)) if args.square else sorted(
        str(path.relative_to(prepared)) for path in (prepared / "z9").glob("*/*") if path.is_dir())
    if not names or any(parse_square_name(name) is None or not (prepared / name).is_dir() for name in names):
        raise ValueError("Select existing prepared z9/x/y directories")
    if not any((prepared / name / "structures.arrow").is_file() for name in names):
        raise ValueError("No structures.arrow in selected squares; run the structures builder first")
    road_names = [name for name in names if (prepared / name / "roads.arrow").is_file()]
    requested = cpu_jobs() if args.workers is None else args.workers
    workers = min(len(road_names), fit_jobs(requested, WORKER_BYTES)) if road_names else 1
    print(
        f"[build-built-up] workers={workers} requested={requested} "
        f"memory_bytes={available_memory_bytes()} worker_bytes={WORKER_BYTES}",
        flush=True,
    )
    totals = Counter()
    with (prepared / ".built-up-build.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if workers == 1:
            results = map(lambda name: bake_square(prepared, name), road_names)
            for name, result in results:
                totals.update(result)
                print(json.dumps({"square": name, **result}), flush=True)
        else:
            with ProcessPoolExecutor(max_workers=workers, initializer=initialize_worker,
                                     initargs=(prepared,)) as executor:
                for name, result in executor.map(
                        bake_square, [prepared] * len(road_names), road_names, chunksize=4):
                    totals.update(result)
                    print(json.dumps({"square": name, **result}), flush=True)
    print(json.dumps({"total": dict(totals)}), flush=True)


if __name__ == "__main__":
    main()
