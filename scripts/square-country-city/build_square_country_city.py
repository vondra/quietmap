"""Bake per-segment geography and per-z9 square-country-city records without changing Arrow geometry."""

import argparse
from collections import Counter
from concurrent.futures import ProcessPoolExecutor, as_completed
import fcntl
import json
import multiprocessing as mp
import os
from pathlib import Path
import struct
import sys
import tempfile

import numpy as np
import pyarrow as pa

from admin_at import AdminResolver

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
from qmgrid import parse_square_name, square_id, square_lonlat_span  # noqa: E402
from prepared_arrow import replace_atomically, rewrite_arrow_batches, segment_midpoints, grid_points  # noqa: E402
from worker_jobs import available_memory_bytes, cpu_jobs, fit_jobs  # noqa: E402

# 20 workers finished the 60 GiB world-build share of square-country-city.
WORKER_BYTES = 2 << 30

COUNTRY_CITY_COLUMNS = {"country_iso": pa.uint16(), "city_id": pa.uint16(), "continent": pa.uint8()}
COUNTRY_CONTRACT = b"country_baked_v1"
LAND_CONTRACT = b"country_land_baked_v1"
_PREPARED = None
_RESOLVER = None


def baked_batch(batch, resolver, contract_key):
    present = [name for name in COUNTRY_CITY_COLUMNS if name in batch.schema.names]
    if present and len(present) != len(COUNTRY_CITY_COLUMNS):
        raise ValueError("Partial country bake; country_iso/city_id/continent must be all-or-none")
    industrial = contract_key == b"industrial_contract"
    if industrial and (batch.schema.metadata or {}).get(b"grid") != b"z30":
        raise ValueError("industrial Arrow requires the z30 grid contract")
    values = (resolver.resolve_land(*grid_points(batch, "centroid")) if industrial
              else resolver.resolve(*segment_midpoints(batch)))
    result = batch
    for name, arrow_type in COUNTRY_CITY_COLUMNS.items():
        array = pa.array(values[name], type=arrow_type)
        index = result.schema.get_field_index(name)
        if index >= 0:
            if result.column(index).type != arrow_type or result.column(index).null_count:
                raise ValueError(f"Invalid existing {name} column")
            result = result.set_column(index, result.schema.field(index), array)
        else:
            result = result.append_column(pa.field(name, arrow_type, nullable=False), array)
    metadata = dict(batch.schema.metadata or {})
    metadata[contract_key] = LAND_CONTRACT if industrial else COUNTRY_CONTRACT
    return result.replace_schema_metadata(metadata)


def bake_file(path, resolver):
    contract_key = (path.stem + "_contract").encode()
    return rewrite_arrow_batches(path, lambda batch: baked_batch(batch, resolver, contract_key))


def expected_contract(path):
    if path.stem == "industrial":
        return b"industrial_contract", LAND_CONTRACT
    return (path.stem + "_contract").encode(), COUNTRY_CONTRACT


def already_baked(path):
    key, expected = expected_contract(path)
    with pa.memory_map(str(path), "r") as source:
        metadata = pa.ipc.open_file(source).schema.metadata or {}
    return metadata.get(key) == expected


def process_square(prepared, resolver, name):
    counts = Counter(roads_rows=0, railways_rows=0, industrial_rows=0, files_changed=0, squares=0)
    for layer in ("roads", "railways", "industrial"):
        path = prepared / name / f"{layer}.arrow"
        if not path.is_file():
            continue
        if already_baked(path):
            continue
        rows, changed = bake_file(path, resolver)
        counts[layer + "_rows"] += rows
        counts["files_changed"] += int(changed)
    square = parse_square_name(name)
    assert square is not None
    write_square_country_city_record(prepared / name, square_country_city_record(resolver, *square))
    counts["squares"] += 1
    return {"square": name, **counts}


def square_country_city_record(resolver, x, y):
    west, north, east, south = square_lonlat_span(x, y)
    lat, lon = (north + south) / 2, (west + east) / 2
    result = resolver.resolve([lat], [lon])
    if not result["country_iso"][0]:
        # dev1 max-share coastal fallback, adapted to the square's interior.
        samples = [(south + (north - south) * fy, west + (east - west) * fx)
                   for fy in (0, 1 / 3, 1 / 2, 2 / 3, 1)
                   for fx in (0, 1 / 3, 1 / 2, 2 / 3, 1)]
        resolved = resolver.resolve([p[0] for p in samples], [p[1] for p in samples])
        shares = Counter(int(code) for code in resolved["country_iso"] if code)
        if shares:
            country = min(shares, key=lambda code: (-shares[code], code.to_bytes(2, "little")))
            index = int(np.flatnonzero(resolved["country_iso"] == country)[0])
            result["country_iso"][0] = country
            result["continent"][0] = resolved["continent"][index]
            # City belongs to the centroid under its resolved country.
            result["city_id"] = resolver.city_ids([lat], [lon], result["country_iso"])
    square = square_id(x, y)
    return struct.pack("<QBHH", square, int(result["continent"][0]),
                       int(result["country_iso"][0]), int(result["city_id"][0]))


def write_square_country_city_record(directory, record):
    path = directory / "square-country-city.bin"
    if path.exists() and path.read_bytes() == record:
        return
    descriptor, name = tempfile.mkstemp(prefix=".square-country-city.", dir=directory)
    try:
        with os.fdopen(descriptor, "wb") as output:
            os.fchmod(output.fileno(), 0o644)
            output.write(record)
            output.flush()
            os.fsync(output.fileno())
        replace_atomically(name, path)
    finally:
        Path(name).unlink(missing_ok=True)


def _init_worker(prepared, boundaries):
    global _PREPARED, _RESOLVER
    _PREPARED = prepared
    _RESOLVER = AdminResolver.from_file(boundaries)


def _process_name(name):
    return process_square(_PREPARED, _RESOLVER, name)


def emit_square(row, totals):
    totals.update({key: value for key, value in row.items() if key != "square"})
    print(json.dumps({"square": row["square"], **totals}), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prepared-dir", type=Path, required=True)
    parser.add_argument("--boundaries", type=Path, required=True)
    parser.add_argument("--square", action="append", help="Repeat to limit the build to selected z9/x/y units")
    parser.add_argument("--jobs", type=int, default=None,
                        help="Worker cap (default: all CPUs that fit memory; 1 keeps the serial path)")
    args = parser.parse_args()
    if args.jobs is not None and args.jobs < 1:
        raise ValueError("--jobs must be >= 1")
    prepared = args.prepared_dir.resolve(strict=True)
    names = args.square or sorted(str(path.relative_to(prepared)) for path in (prepared / "z9").glob("*/*") if path.is_dir())
    if not names:
        raise ValueError("No prepared z9 squares; run the vector extract first")
    squares = [(name, parse_square_name(name)) for name in names]
    if any(square is None or not (prepared / name).is_dir() for name, square in squares):
        raise ValueError("Every selected square must be an existing z9/x/y directory")
    totals = Counter()
    with (prepared / ".square-country-city-build.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        requested = cpu_jobs() if args.jobs is None else args.jobs
        jobs = min(len(squares), fit_jobs(requested, WORKER_BYTES))
        print(
            f"[square-country-city] jobs={jobs} requested={requested} "
            f"memory_bytes={available_memory_bytes()} worker_bytes={WORKER_BYTES}",
            flush=True,
        )
        if jobs == 1:
            resolver = AdminResolver.from_file(args.boundaries)
            for name, _square in squares:
                emit_square(process_square(prepared, resolver, name), totals)
            return
        context = mp.get_context("spawn")
        with ProcessPoolExecutor(
            max_workers=jobs, mp_context=context,
            initializer=_init_worker, initargs=(prepared, args.boundaries.resolve(strict=True)),
        ) as pool:
            futures = [pool.submit(_process_name, name) for name, _square in squares]
            for future in as_completed(futures):
                emit_square(future.result(), totals)


if __name__ == "__main__":
    main()
