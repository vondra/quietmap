"""Derive structure coverage from prepared squares and the complete Overture source cache."""

import math
from pathlib import Path

import pyarrow.parquet as pq

import qmgrid


def degree_name(lat, lon):
    return f"{'N' if lat >= 0 else 'S'}{abs(lat):02d}{'E' if lon >= 0 else 'W'}{abs(lon):03d}"


def overture_sources(parquet_dir, square):
    lon0, lat_top, lon1, lat_bot = qmgrid.square_lonlat_span(*square)
    if square[1] == 0:
        lat_top = 90
    if square[1] == qmgrid.Z9_AXIS - 1:
        lat_bot = -90
    for lat in range(math.floor(lat_bot), min(90, math.floor(lat_top) + 1)):
        for lon in range(math.floor(lon0), math.ceil(lon1)):
            source = Path(parquet_dir) / f"{degree_name(lat, lon)}.parquet"
            if not source.is_file():
                raise SystemExit(
                    f"{qmgrid.square_name(*square)}: Overture parquet {source} is missing — "
                    "run scripts/overture/download-overture-world.sh first")
            yield lat, lon, source


def world_squares(prepared_dir, parquet_dir):
    sources = {degree_name(lat, lon): (lat, lon)
               for lat in range(-90, 90) for lon in range(-180, 180)}
    cached = {path.stem: path for path in Path(parquet_dir).glob("*.parquet")}
    missing, extra = sources.keys() - cached.keys(), cached.keys() - sources.keys()
    if missing or extra:
        raise ValueError(f"Incomplete world Overture cache: {len(missing)} missing "
                         f"{sorted(missing)[:10]}, {len(extra)} unexpected {sorted(extra)[:10]}")
    squares = set()
    for path in (Path(prepared_dir) / "z9").glob("*/*"):
        if not path.is_dir():
            continue
        name = f"z9/{path.parent.name}/{path.name}"
        square = qmgrid.parse_square_name(name)
        if square is None or qmgrid.square_name(*square) != name:
            raise ValueError(f"Noncanonical prepared square: {path}")
        squares.add(square)
    for name, (lat, lon) in sources.items():
        if pq.ParquetFile(cached[name]).metadata.num_rows == 0:
            continue
        # A degree tile's owner candidates also include empty squares; this
        # conservative cover avoids decoding billions of footprints twice.
        x0 = math.floor((lon + 180) / qmgrid.Z9_SPAN_DEG)
        x1 = math.ceil((lon + 181) / qmgrid.Z9_SPAN_DEG)
        y0 = qmgrid.square_of(math.nextafter(lat + 1, -math.inf), lon)[1]
        y1 = qmgrid.square_of(lat, lon)[1]
        squares.update((x, y) for x in range(x0, x1) for y in range(y0, y1 + 1))
    return [qmgrid.square_name(x, y) for x, y in sorted(squares)]
