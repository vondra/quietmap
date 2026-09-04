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
from qmgrid import (QUANTUM_M, RADIUS_M, parse_square_name, square_id,
                    square_lonlat_span, wrapped_longitude_midpoint)  # noqa: E402

ADMIN_COLUMNS = {"country_iso": pa.uint16(), "city_id": pa.uint16(), "continent": pa.uint8()}
COUNTRY_CONTRACT = b"country_baked_v1"


def replace_atomically(temporary, path):
    os.replace(temporary, path)
    descriptor = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def segment_midpoints(batch):
    coordinates = {}
    for name in ("start_gx", "start_gy", "end_gx", "end_gy"):
        index = batch.schema.get_field_index(name)
        if index < 0 or batch.column(index).type != pa.int32() or batch.column(index).null_count:
            raise ValueError(f"{name} must be a non-null Int32 grid column")
        coordinates[name] = batch.column(index).to_numpy().astype(np.float64)
    def lonlat(prefix):
        x = (coordinates[prefix + "_gx"] - (1 << 29)) * QUANTUM_M
        y = (coordinates[prefix + "_gy"] - (1 << 29)) * QUANTUM_M
        return np.degrees(x / RADIUS_M), np.degrees(2 * np.arctan(np.exp(y / RADIUS_M)) - np.pi / 2)
    start_lon, start_lat = lonlat("start")
    end_lon, end_lat = lonlat("end")
    longitude = wrapped_longitude_midpoint(start_lon, end_lon)
    return (start_lat + end_lat) / 2, longitude


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
    original_stat = path.stat()
    descriptor, temporary_name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temporary = Path(temporary_name)
    changed, rows = False, 0
    try:
        with os.fdopen(descriptor, "wb") as output, pa.memory_map(str(path), "r") as source:
            reader = pa.ipc.open_file(source)
            writer = None
            try:
                # Keep the original batch boundaries, so qm_batch_bboxes stays exact.
                batches = reader.num_record_batches
                if not batches:
                    raise ValueError(f"{path}: no record batches; refusing an unverified bake")
                for i in range(batches):
                    original = reader.get_batch(i)
                    baked = baked_batch(original, resolver, contract_key)
                    if writer is None:
                        writer = pa.ipc.new_file(output, baked.schema)
                    writer.write_batch(baked)
                    changed |= not original.equals(baked, check_metadata=True)
                    rows += baked.num_rows
            finally:
                if writer is not None:
                    writer.close()
            output.flush()
            os.fsync(output.fileno())
        if changed:
            # Verify before replace: a failed write never destroys the extracted input.
            with pa.memory_map(str(temporary), "r") as source:
                check = pa.ipc.open_file(source)
                if check.num_record_batches != batches or sum(check.get_batch(i).num_rows for i in range(batches)) != rows:
                    raise ValueError(f"{path}: bake lost rows or changed spatial batches")
                if check.schema.metadata.get(contract_key) != COUNTRY_CONTRACT:
                    raise ValueError(f"{path}: bake lost its contract")
            current = path.stat()
            if (current.st_ino, current.st_size, current.st_mtime_ns) != (original_stat.st_ino, original_stat.st_size, original_stat.st_mtime_ns):
                raise RuntimeError(f"{path}: concurrent writer changed the input")
            os.chmod(temporary, original_stat.st_mode & 0o777)
            replace_atomically(temporary, path)
        return rows, changed
    finally:
        temporary.unlink(missing_ok=True)


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
    return square, struct.pack("<QBHH", square, int(result["continent"][0]),
                               int(result["country_iso"][0]), int(result["city_id"][0]))


def write_admin_record(prepared, square, record):
    directory = prepared / "admin" / str(square)
    directory.mkdir(parents=True, exist_ok=True)
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
            identity, record = square_admin(resolver, *square)
            write_admin_record(prepared, identity, record)
            totals["squares"] += 1
            print(json.dumps({"square": name, **totals}), flush=True)


if __name__ == "__main__":
    main()
