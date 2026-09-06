"""Batch-preserving atomic Arrow enrichment and grid segment midpoints."""

import os
from pathlib import Path
import tempfile

import numpy as np
import pyarrow as pa

from qmgrid import QUANTUM_M, RADIUS_M, wrapped_longitude_midpoint


def replace_atomically(temporary, path):
    os.replace(temporary, path)
    descriptor = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def grid_points(batch, prefix):
    coordinates = []
    for axis in ("gx", "gy"):
        name = prefix + "_" + axis
        index = batch.schema.get_field_index(name)
        if index < 0 or batch.column(index).type != pa.int32() or batch.column(index).null_count:
            raise ValueError(f"{name} must be a non-null Int32 grid column")
        coordinates.append((batch.column(index).to_numpy().astype(np.float64) - (1 << 29)) * QUANTUM_M)
    x, y = coordinates
    return np.degrees(2 * np.arctan(np.exp(y / RADIUS_M)) - np.pi / 2), np.degrees(x / RADIUS_M)


def segment_midpoints(batch):
    start_lat, start_lon = grid_points(batch, "start")
    end_lat, end_lon = grid_points(batch, "end")
    return (start_lat + end_lat) / 2, wrapped_longitude_midpoint(start_lon, end_lon)


def rewrite_arrow_batches(path, transform):
    original_stat = path.stat()
    descriptor, temporary_name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temporary = Path(temporary_name)
    changed, rows = False, 0
    try:
        with os.fdopen(descriptor, "wb") as output, pa.memory_map(str(path), "r") as source:
            reader = pa.ipc.open_file(source)
            schema = reader.schema
            writer = None
            try:
                # Batch boundaries belong to qm_batch_bboxes and cannot move.
                batches = reader.num_record_batches
                if not batches:
                    raise ValueError(f"{path}: no record batches; refusing an unverified bake")
                batch_rows = []
                for i in range(batches):
                    original = reader.get_batch(i)
                    baked = transform(original)
                    if baked.num_rows != original.num_rows:
                        raise ValueError(f"{path}: enrichment changed a spatial batch's row count")
                    if writer is None:
                        schema = baked.schema
                        writer = pa.ipc.new_file(output, schema)
                    writer.write_batch(baked)
                    batch_rows.append(baked.num_rows)
                    changed |= not original.equals(baked, check_metadata=True)
                    rows += baked.num_rows
            finally:
                if writer is not None:
                    writer.close()
            output.flush()
            os.fsync(output.fileno())
        if changed:
            # A failed write never destroys the extracted input.
            with pa.memory_map(str(temporary), "r") as source:
                check = pa.ipc.open_file(source)
                if (check.num_record_batches != batches
                        or [check.get_batch(i).num_rows for i in range(batches)] != batch_rows
                        or not check.schema.equals(schema, check_metadata=True)):
                    raise ValueError(f"{path}: enrichment lost its schema or spatial batches")
            current = path.stat()
            if (current.st_ino, current.st_size, current.st_mtime_ns) != (original_stat.st_ino, original_stat.st_size, original_stat.st_mtime_ns):
                raise RuntimeError(f"{path}: concurrent writer changed the input")
            os.chmod(temporary, original_stat.st_mode & 0o777)
            replace_atomically(temporary, path)
        return rows, changed
    finally:
        temporary.unlink(missing_ok=True)
