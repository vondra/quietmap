"""Bake road built_up (0 unknown, 1 rural, 2 urban) from prepared structures_v4."""

import argparse
from concurrent.futures import ProcessPoolExecutor
from collections import Counter
import fcntl
import json
from pathlib import Path
import sys

import pyarrow as pa

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
from prepared_arrow import rewrite_arrow_batches, segment_midpoints  # noqa: E402
from qmgrid import parse_square_name  # noqa: E402
from building_footprints import BuildingFootprintSampler  # noqa: E402


def bake_file(path, sampler):
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
            return batch.append_column(pa.field("built_up", pa.uint8(), nullable=False), array)
        if batch.column(index).type != pa.uint8() or batch.column(index).null_count:
            raise ValueError(f"{path}: built_up must be a non-null UInt8 column")
        return batch.set_column(index, batch.schema.field(index), array)

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
    parser.add_argument("--workers", type=int, default=1,
                        help="Independent square workers (choose only from a measured memory-cgroup admission)")
    args = parser.parse_args()
    if not 1 <= args.workers <= 16:
        parser.error("--workers must be in 1..16")
    prepared = args.prepared_dir.resolve(strict=True)
    names = sorted(set(args.square)) if args.square else sorted(
        str(path.relative_to(prepared)) for path in (prepared / "z9").glob("*/*") if path.is_dir())
    if not names or any(parse_square_name(name) is None or not (prepared / name).is_dir() for name in names):
        raise ValueError("Select existing prepared z9/x/y directories")
    if not any((prepared / name / "structures.arrow").is_file() for name in names):
        raise ValueError("No structures.arrow in selected squares; run the structures builder first")
    road_names = [name for name in names if (prepared / name / "roads.arrow").is_file()]
    totals = Counter()
    with (prepared / ".built-up-build.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if args.workers == 1:
            results = map(lambda name: bake_square(prepared, name), road_names)
            for name, result in results:
                totals.update(result)
                print(json.dumps({"square": name, **result}), flush=True)
        else:
            with ProcessPoolExecutor(max_workers=args.workers, initializer=initialize_worker,
                                     initargs=(prepared,)) as executor:
                for name, result in executor.map(
                        bake_square, [prepared] * len(road_names), road_names, chunksize=4):
                    totals.update(result)
                    print(json.dumps({"square": name, **result}), flush=True)
    print(json.dumps({"total": dict(totals)}), flush=True)


if __name__ == "__main__":
    main()
