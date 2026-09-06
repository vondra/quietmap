"""Bake per-segment geography and per-z9 admin records without changing Arrow geometry."""

import argparse
from collections import Counter
import fcntl
import json
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
from prepared_arrow import replace_atomically, rewrite_arrow_batches, segment_midpoints  # noqa: E402

ADMIN_COLUMNS = {"country_iso": pa.uint16(), "city_id": pa.uint16(), "continent": pa.uint8()}
COUNTRY_CONTRACT = b"country_baked_v1"


def baked_batch(batch, resolver, contract_key):
    present = [name for name in ADMIN_COLUMNS if name in batch.schema.names]
    if present and len(present) != len(ADMIN_COLUMNS):
        raise ValueError("Partial country bake; country_iso/city_id/continent must be all-or-none")
    values = resolver.resolve(*segment_midpoints(batch))
    result = batch
    for name, arrow_type in ADMIN_COLUMNS.items():
        array = pa.array(values[name], type=arrow_type)
        index = result.schema.get_field_index(name)
        if index >= 0:
            if result.column(index).type != arrow_type or result.column(index).null_count:
                raise ValueError(f"Invalid existing {name} column")
            result = result.set_column(index, result.schema.field(index), array)
        else:
            result = result.append_column(pa.field(name, arrow_type, nullable=False), array)
    metadata = dict(batch.schema.metadata or {})
    metadata[contract_key] = COUNTRY_CONTRACT
    return result.replace_schema_metadata(metadata)


def bake_file(path, resolver):
    contract_key = (path.stem + "_contract").encode()
    return rewrite_arrow_batches(path, lambda batch: baked_batch(batch, resolver, contract_key))


def square_admin(resolver, x, y):
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


def write_admin_record(directory, record):
    path = directory / "admin.bin"
    if path.exists() and path.read_bytes() == record:
        return
    descriptor, name = tempfile.mkstemp(prefix=".admin.", dir=directory)
    try:
        with os.fdopen(descriptor, "wb") as output:
            os.fchmod(output.fileno(), 0o644)
            output.write(record)
            output.flush()
            os.fsync(output.fileno())
        replace_atomically(name, path)
    finally:
        Path(name).unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prepared-dir", type=Path, required=True)
    parser.add_argument("--boundaries", type=Path, required=True)
    parser.add_argument("--square", action="append", help="Repeat to limit the build to selected z9/x/y units")
    args = parser.parse_args()
    prepared = args.prepared_dir.resolve(strict=True)
    names = args.square or sorted(str(path.relative_to(prepared)) for path in (prepared / "z9").glob("*/*") if path.is_dir())
    if not names:
        raise ValueError("No prepared z9 squares; run the vector extract first")
    squares = [(name, parse_square_name(name)) for name in names]
    if any(square is None or not (prepared / name).is_dir() for name, square in squares):
        raise ValueError("Every selected square must be an existing z9/x/y directory")
    resolver = AdminResolver.from_file(args.boundaries)
    totals = Counter()
    with (prepared / ".admin-build.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        for name, square in squares:
            for layer in ("roads", "railways"):
                path = prepared / name / f"{layer}.arrow"
                if path.is_file():
                    rows, changed = bake_file(path, resolver)
                    totals[layer + "_rows"] += rows
                    totals["files_changed"] += changed
            assert square is not None
            write_admin_record(prepared / name, square_admin(resolver, *square))
            totals["squares"] += 1
            print(json.dumps({"square": name, **totals}), flush=True)


if __name__ == "__main__":
    main()
