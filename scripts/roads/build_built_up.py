"""Bake road built_up (0 unknown, 1 rural, 2 urban) from prepared structures_v4."""

import argparse
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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prepared-dir", type=Path, required=True)
    parser.add_argument("--square", action="append", help="Repeat to select existing z9/x/y units")
    args = parser.parse_args()
    prepared = args.prepared_dir.resolve(strict=True)
    names = sorted(set(args.square)) if args.square else sorted(
        str(path.relative_to(prepared)) for path in (prepared / "z9").glob("*/*") if path.is_dir())
    if not names or any(parse_square_name(name) is None or not (prepared / name).is_dir() for name in names):
        raise ValueError("Select existing prepared z9/x/y directories")
    if not any((prepared / name / "structures.arrow").is_file() for name in names):
        raise ValueError("No structures.arrow in selected squares; run the structures builder first")
    sampler = BuildingFootprintSampler(prepared)
    totals = Counter()
    with (prepared / ".built-up-build.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        for name in names:
            path = prepared / name / "roads.arrow"
            if not path.is_file():
                continue
            result = bake_file(path, sampler)
            totals.update(result)
            print(json.dumps({"square": name, **result}), flush=True)
    print(json.dumps({"total": dict(totals)}), flush=True)


if __name__ == "__main__":
    main()
