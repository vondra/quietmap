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

import glob
import io
import zipfile

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
# `sources::SOURCES` ids of the EMODnet 2024 vessel density product and of GFW AIS presence.
SOURCE_ID_EMODNET_2024 = 9901
SOURCE_ID_GFW_PRESENCE = 9902
# GFW report zips (scripts/ships/download_gfw.py) hold one Int32 GeoTIFF of vessel-hours per 0.01°
# cell, or for nearly empty tiles the CSV report with one row per vessel and cell.
GFW_TIF_MEMBER = "layer-activity-data-0/public-global-presence-v4.0.tif"
GFW_CSV_MEMBER = "layer-activity-data-0/public-global-presence-v4.0.csv"
GFW_NODATA = 999999
GFW_CELL_DEG = 0.01
HOURS_PER_MONTH = 365.25 * 24.0 / 12.0  # emission/ships.rs::HOURS_PER_MONTH
METRES_PER_DEGREE = 111_320.0
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
        "coverage": {"mask": mask, "crs": crs, "affine": affine},
    }


def covered_by(coverage, lon, lat):
    """True where a product samples the cell centre (its raster is valid there)."""
    xs, ys = warp_transform("EPSG:4326", coverage["crs"], np.asarray(lon), np.asarray(lat))
    cols, rows = (~coverage["affine"]) * (np.asarray(xs), np.asarray(ys))
    cols, rows = np.floor(cols).astype(np.int64), np.floor(rows).astype(np.int64)
    mask = coverage["mask"]
    inside = (rows >= 0) & (rows < mask.shape[0]) & (cols >= 0) & (cols < mask.shape[1])
    hit = np.zeros(len(cols), dtype=bool)
    hit[inside] = mask[rows[inside], cols[inside]]
    return hit


def gfw_tile_hours(path):
    """Vessel-hours per 0.01° cell of one GFW report zip as {(lon_centre, lat_centre): hours};
    empty for a 0-byte file (no vessels)."""
    payload = Path(path).read_bytes()
    cells = {}
    if not payload:
        return cells
    with zipfile.ZipFile(io.BytesIO(payload)) as archive:
        names = set(archive.namelist())
        if GFW_TIF_MEMBER in names:
            with rasterio.open(io.BytesIO(archive.read(GFW_TIF_MEMBER))) as dataset:
                hours = dataset.read(1)
                rows, cols = np.nonzero((hours != GFW_NODATA) & (hours > 0))
                lon, lat = dataset.transform * (cols + 0.5, rows + 0.5)
                for x, y, value in zip(np.asarray(lon), np.asarray(lat), hours[rows, cols]):
                    key = (round(float(x), 3), round(float(y), 3))
                    cells[key] = cells.get(key, 0.0) + float(value)
        elif GFW_CSV_MEMBER in names:
            import csv
            for row in csv.DictReader(io.TextIOWrapper(io.BytesIO(archive.read(GFW_CSV_MEMBER)), encoding="utf-8")):
                key = (round(float(row["Lon"]), 3), round(float(row["Lat"]), 3))
                cells[key] = cells.get(key, 0.0) + float(row["Vessel Presence Hours"])
        else:
            raise ValueError(f"GFW report without a TIF or CSV member: {path}")
    return cells


def read_gfw(directory, exclude=None):
    """Cells of the GFW world download: hours per month by class (leisure has no GFW type),
    skipping cell centres that `exclude` (an EMODnet coverage) samples."""
    directory = Path(directory)
    window = json.loads((directory / "window.json").read_text())
    # vessel-hours over the window → mean vessel-hours per month (30.4375 days).
    per_month = HOURS_PER_MONTH / 24.0 / float(window["days"])
    parts = {key: [] for key in ("lon", "lat", "area_m2", "hours_large", "hours_work", "hours_leisure")}
    tile_hours_total = 0.0
    cell_side_m = GFW_CELL_DEG * METRES_PER_DEGREE
    for large_path in sorted(glob.glob(str(directory / "*-large.zip"))):
        work_path = large_path[: -len("-large.zip")] + "-work.zip"
        large = gfw_tile_hours(large_path)
        work = gfw_tile_hours(work_path)
        keys = sorted(set(large) | set(work))
        if not keys:
            continue
        hours_large = np.array([large.get(key, 0.0) for key in keys]) * per_month
        hours_work = np.array([work.get(key, 0.0) for key in keys]) * per_month
        total = hours_large + hours_work
        tile_hours_total += float(total.sum())
        lon = np.array([key[0] for key in keys])
        lat = np.array([key[1] for key in keys])
        keep = total >= MIN_CELL_HOURS_PER_MONTH
        if exclude is not None:
            keep &= ~covered_by(exclude, lon, lat)
        parts["lon"].append(lon[keep])
        parts["lat"].append(lat[keep])
        parts["area_m2"].append(cell_side_m * cell_side_m * np.cos(np.radians(lat[keep])))
        parts["hours_large"].append(hours_large[keep])
        parts["hours_work"].append(hours_work[keep])
        parts["hours_leisure"].append(np.zeros(int(keep.sum())))
    cells = {key: (np.concatenate(values) if values else np.zeros(0)) for key, values in parts.items()}
    cells["source_id"] = np.full(len(cells["lon"]), SOURCE_ID_GFW_PRESENCE, dtype=np.uint16)
    cells["raster_hours_total"] = tile_hours_total
    cells["raster_hours_kept"] = float(cells["hours_large"].sum() + cells["hours_work"].sum())
    return cells


def concatenate(*cell_sets):
    keys = ("lon", "lat", "area_m2", "hours_large", "hours_work", "hours_leisure", "source_id")
    return {key: np.concatenate([cells[key] for cells in cell_sets]) for key in keys}


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
    parser.add_argument("--gfw-dir", help="GFW world download (download_gfw.py output); used where EMODnet has no sample")
    args = parser.parse_args()
    emodnet = read_emodnet(args.emodnet_dir)
    report = {"emodnet": {key: emodnet[key] for key in ("raster_hours_total", "raster_hours_kept")}, "emodnet_cells": int(len(emodnet["lon"]))}
    cells = emodnet
    if args.gfw_dir:
        gfw = read_gfw(args.gfw_dir, exclude=emodnet["coverage"])
        report["gfw"] = {key: gfw[key] for key in ("raster_hours_total", "raster_hours_kept")}
        report["gfw_cells"] = int(len(gfw["lon"]))
        cells = concatenate(emodnet, gfw)
    report.update(write_prepared(cells, args.prepared_dir))
    print(json.dumps(report))


if __name__ == "__main__":
    main()
