"""Require complete Overture inputs and every z9 structure output."""

import math
from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq

import qmgrid


def degree_name(lat, lon):
    return f"{'N' if lat >= 0 else 'S'}{abs(lat):02d}{'E' if lon >= 0 else 'W'}{abs(lon):03d}"


def _touched_degree_tiles(cache_dir, square):
    """Every 1-degree tile path a square's span touches, with its (lat, lon)."""
    lon0, lat_top, lon1, lat_bot = qmgrid.square_lonlat_span(*square)
    if square[1] == 0:
        lat_top = 90
    if square[1] == qmgrid.Z9_AXIS - 1:
        lat_bot = -90
    for lat in range(math.floor(lat_bot), min(90, math.floor(lat_top) + 1)):
        for lon in range(math.floor(lon0), math.ceil(lon1)):
            yield lat, lon, Path(cache_dir) / f"{degree_name(lat, lon)}.parquet"


def overture_sources(parquet_dir, square):
    for lat, lon, source in _touched_degree_tiles(parquet_dir, square):
        if not source.is_file():
            raise SystemExit(
                f"{qmgrid.square_name(*square)}: Overture parquet {source} is missing — "
                "run scripts/overture/download-overture-world.sh first")
        yield lat, lon, source


def official_tile_sources(cache_dir, square):
    """The existing cache tiles a square's span touches. Official caches cover
    only surveyed countries, so missing tiles are absent data, not an error."""
    for lat, lon, source in _touched_degree_tiles(cache_dir, square):
        if source.is_file():
            yield lat, lon, source


def write_official_cache(rows_by_tile, cache_dir, schema, contract_key, contract_version):
    """One parquet per 1-degree tile from {tile_name: {column: [...]}}."""
    cache_dir = Path(cache_dir)
    cache_dir.mkdir(parents=True, exist_ok=True)
    for tile, columns in sorted(rows_by_tile.items()):
        table = pa.table({name: columns[name] for name in schema.names}, schema=schema)
        meta = dict(table.schema.metadata or {})
        meta[contract_key] = contract_version
        pq.write_table(table.replace_schema_metadata(meta), cache_dir / f"{tile}.parquet")


def accumulate_official_cache(by_tile, source, cache_dir, schema, contract_key,
                              contract_version):
    """Merge new tile columns with the cached tiles. A re-run replaces its own
    source's rows for the geometries it re-supplies, so a corrected height
    lands and a repeated run writes identical bytes; every other row (other
    sources, unsupplied geometries) stays, so partial top-ups accumulate."""
    cache_dir = Path(cache_dir)
    for tile, columns in by_tile.items():
        path = cache_dir / f"{tile}.parquet"
        if not path.is_file():
            continue
        supplied = set(columns["geometry"])
        for row in pq.read_table(path).to_pylist():
            if row["source"] == source and row["geometry"] in supplied:
                continue
            for name in schema.names:
                columns[name].append(row[name])
    write_official_cache(by_tile, cache_dir, schema, contract_key, contract_version)


def world_squares(parquet_dir):
    sources = {degree_name(lat, lon)
               for lat in range(-90, 90) for lon in range(-180, 180)}
    cached = {path.stem for path in Path(parquet_dir).glob("*.parquet")}
    missing, extra = sources - cached, cached - sources
    if missing or extra:
        raise ValueError(f"Incomplete world Overture cache: {len(missing)} missing "
                         f"{sorted(missing)[:10]}, {len(extra)} unexpected {sorted(extra)[:10]}")
    return [qmgrid.square_name(x, y)
            for x in range(qmgrid.Z9_AXIS) for y in range(qmgrid.Z9_AXIS)]
