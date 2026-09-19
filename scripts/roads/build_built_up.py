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

import numpy as np
import pyarrow as pa

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
from prepared_arrow import rewrite_arrow_batches, segment_midpoints  # noqa: E402
from qmgrid import parse_square_name, square_name, Z9_AXIS  # noqa: E402
from building_footprints import built_up_classes  # noqa: E402
from worker_jobs import available_memory_bytes, cpu_jobs  # noqa: E402

# A worker's peak anonymous memory stays under this base plus its roads and owner structures
# bytes. Halo cells add only the footprints within one window of the owner's rows; the rest of a
# neighbour file is memory-mapped, reclaimable page cache. Measured 2026-09-19 on eleven squares:
# 51 MiB of inputs peaks at 124 MiB, Tokyo (1122 MiB, the world's heaviest) at 883 MiB, and its
# 91 MiB western neighbour, which maps Tokyo's 1 GiB file, at 179 MiB.
WORKER_BASE_BYTES = 256 << 20


@cache
def classification_code_identity():
    files = (Path(__file__), Path(__file__).with_name('building_footprints.py'),
             Path(__file__).with_name('footprint_polygon_areas.py'),
             Path(__file__).resolve().parents[1] / 'lib/prepared_arrow.py',
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


def unbaked_road_midpoints(path, identity):
    with pa.memory_map(str(path), 'r') as source:
        reader = pa.ipc.open_file(source)
        if (reader.schema.metadata or {}).get(b'qm_built_up_inputs') == identity:
            return None
        if (reader.schema.metadata or {}).get(b"grid") != b"z30":
            raise ValueError(f"{path}: expected grid z30 roads")
        midpoints = [segment_midpoints(reader.get_batch(i)) for i in range(reader.num_record_batches)]
    return [np.concatenate(axis) for axis in zip(*midpoints)] if midpoints else [np.zeros(0), np.zeros(0)]


def bake_file(path, prepared_dir):
    identity = classification_inputs(path)
    midpoints = unbaked_road_midpoints(path, identity)
    if midpoints is None:
        return {'files_changed': 0, 'files_skipped': 1}
    classes = built_up_classes(prepared_dir, *midpoints)
    baked_rows = 0

    def bake_batch(batch):
        nonlocal baked_rows
        array = pa.array(classes[baked_rows:baked_rows + batch.num_rows], type=pa.uint8())
        baked_rows += batch.num_rows
        index = batch.schema.get_field_index("built_up")
        if index < 0:
            batch = batch.append_column(pa.field("built_up", pa.uint8(), nullable=False), array)
        else:
            if batch.column(index).type != pa.uint8() or batch.column(index).null_count:
                raise ValueError(f"{path}: built_up must be a non-null UInt8 column")
            batch = batch.set_column(index, batch.schema.field(index), array)
        return batch.replace_schema_metadata({**(batch.schema.metadata or {}), b'qm_built_up_inputs': identity})

    rows, changed = rewrite_arrow_batches(path, bake_batch)
    unknown, rural, urban = np.bincount(classes, minlength=3).tolist()
    return {"rows": rows, "unknown": unknown, "rural": rural, "urban": urban, "files_changed": int(changed)}


def bake_square(prepared_dir, name):
    return name, bake_file(Path(prepared_dir) / name / "roads.arrow", prepared_dir)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prepared-dir", type=Path, required=True)
    parser.add_argument("--square", action="append", help="Repeat to select existing z9/x/y units")
    parser.add_argument("--workers", type=int, default=None,
                        help="Worker cap (default: all CPUs that fit memory)")
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
    # Heaviest first, one square per task: no worker ends the run alone behind a queue of cities,
    # and the first tasks are the largest set that ever runs together, so they size the pool.
    worker_bytes = {name: WORKER_BASE_BYTES + sum(
        (prepared / name / file).stat().st_size for file in ("roads.arrow", "structures.arrow")
        if (prepared / name / file).is_file())
        for name in names if (prepared / name / "roads.arrow").is_file()}
    road_names = sorted(worker_bytes, key=lambda name: (-worker_bytes[name], name))
    requested = cpu_jobs() if args.workers is None else args.workers
    # The parent holds the same imports as an idle worker.
    memory_bytes, workers = available_memory_bytes() - WORKER_BASE_BYTES, 0
    while workers < min(requested, len(road_names)) and worker_bytes[road_names[workers]] <= memory_bytes:
        memory_bytes -= worker_bytes[road_names[workers]]
        workers += 1
    if road_names and not workers:
        raise MemoryError(f"{road_names[0]} needs {worker_bytes[road_names[0]]} bytes beside the parent; "
                          f"{available_memory_bytes()} available")
    print(f"[build-built-up] workers={workers} requested={requested} "
          f"memory_bytes={available_memory_bytes()} unclaimed_memory_bytes={memory_bytes}", flush=True)
    totals = Counter()
    with (prepared / ".built-up-build.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        with ProcessPoolExecutor(max_workers=workers or 1) as executor:
            for name, result in executor.map(bake_square, [prepared] * len(road_names), road_names):
                totals.update(result)
                print(json.dumps({"square": name, **result}), flush=True)
    print(json.dumps({"total": dict(totals)}), flush=True)


if __name__ == "__main__":
    main()
