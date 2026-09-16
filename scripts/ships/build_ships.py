#!/usr/bin/env python3
"""Write `ships.arrow` per z9 square from AIS vessel-density rasters: one row per water cell
with its mean vessel-hours per month by acoustic class (EMODnet 2024 European seas today)."""

import argparse
import base64
import json
import os
from pathlib import Path
import struct
import sys
import tempfile

import numpy as np
import pyarrow as pa
import rasterio
from rasterio.warp import transform as warp_transform

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
from prepared_arrow import replace_atomically  # noqa: E402
import qmgrid  # noqa: E402

# EMODnet vessel-density ship types (method report v1.5 Table 2) grouped into the three
# acoustic classes of engine/noise-compute/src/emission/ships.rs.
EMODNET_CLASS_TYPES = {
    "large": ("06", "08", "09", "10", "11", "12"),   # high speed craft, passenger, cargo, tanker, military, unknown
    "work": ("00", "01", "02", "03", "07"),          # other, fishing, service, dredging, tug
    "leisure": ("04", "05"),                         # sailing, pleasure craft
}
CLASSES = ("large", "work", "leisure")
EMODNET_FILE = "vesseldensity_{code}avg_2024_laea.tif"
EMODNET_CELL_AREA_M2 = 1_000_000.0  # native 1 km ETRS89-LAEA (equal-area) cells
# `sources::SOURCES` id of the EMODnet 2024 vessel density product.
SOURCE_ID_EMODNET_2024 = 9901
# A cell below 0.5 vessel-hours per month is at most 76 dB(A) (large ships) — under 10 dB(A)
# at its own edge and nothing at 1 km; dropping it keeps 1.65 M of 13.7 M positive cells.
MIN_CELL_HOURS_PER_MONTH = 0.5
CONTRACT_KEY = b"ships_contract"
CONTRACT_VERSION = b"ships_v1"
MAX_ROWS_PER_BLOCK_BATCH = 4096  # arrow_batching::MAX_ROWS_PER_BLOCK_BATCH
BLOCK_ZOOM = 14
BLOCK_RECORD = struct.Struct("<HHddddff")

SCHEMA = pa.schema([
    pa.field("centroid_gx", pa.int32(), nullable=False),
    pa.field("centroid_gy", pa.int32(), nullable=False),
    pa.field("area_m2", pa.float32(), nullable=False),
    pa.field("hours_large", pa.float32(), nullable=False),
    pa.field("hours_work", pa.float32(), nullable=False),
    pa.field("hours_leisure", pa.float32(), nullable=False),
    pa.field("source_id", pa.uint16(), nullable=False),
], metadata={b"grid": b"z30", CONTRACT_KEY: CONTRACT_VERSION})


def read_emodnet(directory):
    """Cells (lon, lat, area_m2, hours by class, source_id) of the EMODnet 2024 annual averages."""
    directory = Path(directory)
    sums = {}
    mask = None
    for name, codes in EMODNET_CLASS_TYPES.items():
        total = None
        for code in codes:
            with rasterio.open(directory / EMODNET_FILE.format(code=code)) as dataset:
                values = dataset.read(1)
                valid = values != dataset.nodata
                if mask is None:
                    mask, crs, affine = valid.copy(), dataset.crs, dataset.transform
                elif dataset.crs != crs or dataset.transform != affine or dataset.shape != mask.shape:
                    raise ValueError(f"EMODnet grid differs: {dataset.name}")
                else:
                    mask |= valid  # a cell is water when any type raster samples it
            values = np.where(valid & (values > 0), values, 0.0).astype(np.float64)
            total = values if total is None else total + values
        sums[name] = total
    hours = sum(sums.values())
    keep = mask & (hours >= MIN_CELL_HOURS_PER_MONTH)
    rows, cols = np.nonzero(keep)
    xs, ys = affine * (cols + 0.5, rows + 0.5)
    lon, lat = warp_transform(crs, "EPSG:4326", np.asarray(xs), np.asarray(ys))
    return {
        "lon": np.asarray(lon), "lat": np.asarray(lat),
        "area_m2": np.full(len(rows), EMODNET_CELL_AREA_M2),
        **{f"hours_{name}": sums[name][keep] for name in CLASSES},
        "source_id": np.full(len(rows), SOURCE_ID_EMODNET_2024, dtype=np.uint16),
        "raster_hours_total": float(np.where(mask, hours, 0.0).sum()),
        "raster_hours_kept": float(hours[keep].sum()),
    }


def mercator_axes(lat, lon, zoom):
    """Vectorised engine `grid::web_mercator_cell_axes` (edge nudging is irrelevant for cell centres)."""
    axis = 1 << zoom
    lon = ((np.asarray(lon) + 180.0) % 360.0) - 180.0
    x = np.clip(np.floor((lon + 180.0) / 360.0 * axis), 0, axis - 1).astype(np.int64)
    clamped = np.radians(np.clip(lat, -qmgrid.MAX_MERCATOR_LAT, qmgrid.MAX_MERCATOR_LAT))
    northing = qmgrid.RADIUS_M * np.log(np.tan(np.pi / 4.0 + clamped / 2.0))
    y = np.clip(np.floor((0.5 - northing / qmgrid.CIRCUMFERENCE_M) * axis), 0, axis - 1).astype(np.int64)
    return x, y


def grid_columns(lon, lat):
    """z30 integer grid of cell centres (engine `grid::lonlat_to_grid`)."""
    x = qmgrid.RADIUS_M * np.radians(lon)
    clamped = np.radians(np.clip(lat, -qmgrid.MAX_MERCATOR_LAT, qmgrid.MAX_MERCATOR_LAT))
    y = qmgrid.RADIUS_M * np.log(np.tan(np.pi / 4.0 + clamped / 2.0))
    gx = np.floor(x / qmgrid.QUANTUM_M).astype(np.int64) + (1 << 29)
    gy = np.floor(y / qmgrid.QUANTUM_M).astype(np.int64) + (1 << 29)
    return gx.astype(np.int32), gy.astype(np.int32)


def square_table(cells, index):
    """One square's rows as z14-blocked record batches plus the `qm_blocks` metadata value."""
    lon, lat = cells["lon"][index], cells["lat"][index]
    bx, by = mercator_axes(lat, lon, BLOCK_ZOOM)
    order = np.lexsort((bx, by))
    lon, lat, bx, by = lon[order], lat[order], bx[order], by[order]
    gx, gy = grid_columns(lon, lat)
    columns = [pa.array(gx, pa.int32()), pa.array(gy, pa.int32()),
               pa.array(cells["area_m2"][index][order].astype(np.float32))]
    columns += [pa.array(cells[f"hours_{name}"][index][order].astype(np.float32)) for name in CLASSES]
    columns.append(pa.array(cells["source_id"][index][order], pa.uint16()))
    records = [b"\x01"]
    batches = []
    start = 0
    while start < len(order):
        end = start
        while end < len(order) and end - start < MAX_ROWS_PER_BLOCK_BATCH and (bx[end], by[end]) == (bx[start], by[start]):
            end += 1
        records.append(BLOCK_RECORD.pack(int(bx[start]), int(by[start]),
                                         float(lat[start:end].min()), float(lon[start:end].min()),
                                         float(lat[start:end].max()), float(lon[start:end].max()), 0.0, 0.0))
        batches.append((start, end - start))
        start = end
    metadata = dict(SCHEMA.metadata)
    metadata[b"qm_blocks"] = base64.b64encode(b"".join(records))
    table = pa.table(columns, schema=SCHEMA.with_metadata(metadata))
    return table, batches


def write_square(path, table, batches):
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(descriptor, "wb") as output:
            with pa.ipc.new_file(output, table.schema) as writer:
                for offset, length in batches:
                    writer.write_batch(table.slice(offset, length).to_batches()[0])
            output.flush()
            os.fsync(output.fileno())
        replace_atomically(temporary, path)
    except BaseException:
        Path(temporary).unlink(missing_ok=True)
        raise


def write_prepared(cells, prepared):
    """Write every square that has cells and remove `ships.arrow` from squares that lost them."""
    prepared = Path(prepared)
    sx, sy = mercator_axes(cells["lat"], cells["lon"], 9)
    key = sy * qmgrid.Z9_AXIS + sx
    order = np.argsort(key, kind="stable")
    keys, starts = np.unique(key[order], return_index=True)
    written = set()
    rows = 0
    for square_key, start, end in zip(keys, starts, list(starts[1:]) + [len(order)]):
        x, y = int(square_key % qmgrid.Z9_AXIS), int(square_key // qmgrid.Z9_AXIS)
        table, batches = square_table(cells, order[start:end])
        write_square(prepared / qmgrid.square_name(x, y) / "ships.arrow", table, batches)
        written.add((x, y))
        rows += table.num_rows
    removed = 0
    for stale in prepared.glob("z9/*/*/ships.arrow"):
        if (int(stale.parent.parent.name), int(stale.parent.name)) not in written:
            stale.unlink()
            removed += 1
    return {"squares": len(written), "rows": rows, "removed_stale_squares": removed,
            "hours_written": float(sum(cells[f"hours_{name}"].sum() for name in CLASSES))}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prepared-dir", required=True, help="prepared year directory holding z9/")
    parser.add_argument("--emodnet-dir", required=True, help="EMODnet 2024 annual-average LAEA GeoTIFFs")
    args = parser.parse_args()
    cells = read_emodnet(args.emodnet_dir)
    report = write_prepared(cells, args.prepared_dir)
    report["raster_hours_total"] = cells["raster_hours_total"]
    report["raster_hours_kept"] = cells["raster_hours_kept"]
    report["cells_kept"] = int(len(cells["lon"]))
    print(json.dumps(report))


if __name__ == "__main__":
    main()
