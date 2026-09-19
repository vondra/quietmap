"""Road settlement density from the Overture screening stock in structures_v4.

Only footprints near a road row are decoded; geometry validity of the rest belongs to the structures builder.
"""

import math
from pathlib import Path
import sys

import numpy as np
import pyarrow as pa

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
import qmgrid  # noqa: E402
from prepared_arrow import grid_to_latlon  # noqa: E402
from footprint_polygon_areas import METRES_PER_DEG_LAT, METRES_PER_DEG_LON_EQ, footprint_areas_m2  # noqa: E402

# dev1's 17-pixel, 1-arcsecond raster window and 2026-08-30 vector calibration.
WINDOW_HALF_DEG = 8.5 / 3600
MIN_BUILT_PIXELS = 8
# Candidate searches reach this far so float rounding cannot lose a member;
# the closed-window comparisons alone decide membership.
SEARCH_REACH_DEG = WINDOW_HALF_DEG + 1e-9
# One decode or candidate expansion stays near the CPU cache: measured faster than larger steps.
ROWS_PER_STEP = 1 << 16
CANDIDATES_PER_STEP = 1 << 18
STOCK_COLUMNS = {"kind": pa.uint8(), "geom": pa.binary(), "osm_id": pa.int64(),
                 "centroid_gx": pa.int32(), "centroid_gy": pa.int32(),
                 "emission_centroid_gx": pa.int32(), "emission_centroid_gy": pa.int32()}


def squares_of(latitudes, longitudes):
    """qmgrid.square_of for arrays, as x * Z9_AXIS + y."""
    x = ((qmgrid.normalize_longitude(longitudes) + 180.0) / 360.0 * qmgrid.Z9_AXIS).astype(np.int64)
    clamped = np.clip(latitudes, -qmgrid.MAX_MERCATOR_LAT, qmgrid.MAX_MERCATOR_LAT)
    y_m = qmgrid.RADIUS_M * np.log(np.tan(np.pi / 4.0 + np.radians(clamped) / 2.0))
    y = ((qmgrid.CIRCUMFERENCE_M / 2.0 - y_m) / qmgrid.CIRCUMFERENCE_M * qmgrid.Z9_AXIS).astype(np.int64)
    return np.minimum(x, qmgrid.Z9_AXIS - 1) * qmgrid.Z9_AXIS + np.minimum(y, qmgrid.Z9_AXIS - 1)


def window_corner_squares(latitudes, longitudes):
    """Centroid-owner z9 cells under the four window corners; a window is far smaller than a cell."""
    return np.stack([squares_of(latitudes + north, longitudes + east)
                     for north in (WINDOW_HALF_DEG, -WINDOW_HALF_DEG)
                     for east in (-WINDOW_HALF_DEG, WINDOW_HALF_DEG)])


def cell_footprints(path, south, north, west, east_span):
    """(latitude, longitude, area) of the stock inside the bounds, or None for an unbuilt cell."""
    try:
        source = pa.memory_map(str(path), "r")
    except FileNotFoundError:
        return None
    with source:
        reader = pa.ipc.open_file(source)
        metadata = reader.schema.metadata or {}
        if metadata.get(b"structures_contract") != b"structures_v4" or metadata.get(b"grid") != b"z30":
            raise ValueError(f"{path}: expected grid z30 structures_v4")
        for name, arrow_type in STOCK_COLUMNS.items():
            index = reader.schema.get_field_index(name)
            if index < 0 or reader.schema.field(index).type != arrow_type:
                raise ValueError(f"{path}: invalid {name} column")
        table = reader.read_all().select(list(STOCK_COLUMNS))
        for name in ("kind", "centroid_gx", "centroid_gy"):
            if table.column(name).null_count:
                raise ValueError(f"{path}: null {name}")
        found = [np.zeros((3, 0))]
        for start in range(0, table.num_rows, ROWS_PER_STEP):
            values = table.slice(start, ROWS_PER_STEP)
            building = values["kind"].to_numpy() == 0
            emission_x, emission_y, from_osm = (
                values[name].is_valid().to_numpy()
                for name in ("emission_centroid_gx", "emission_centroid_gy", "osm_id"))
            if (building & (emission_x != emission_y)).any():
                raise ValueError(f"{path}: partial emission centroid")
            # Matched rows retain Overture geometry; OSM-only rows do not.
            stock = np.flatnonzero(building & ~(from_osm & ~emission_x))
            lat, lon = grid_to_latlon(values["centroid_gx"].to_numpy()[stock], values["centroid_gy"].to_numpy()[stock])
            lon = qmgrid.normalize_longitude(lon)
            near = (lat >= south) & (lat <= north) & ((lon - west) % 360.0 <= east_span)
            # Only the taken geometry is ever read: a neighbour cell costs its centroid columns, not its file.
            found.append(np.stack([lat[near], lon[near], footprint_areas_m2(
                values["geom"].take(pa.array(stock[near])).combine_chunks())]))
    return np.concatenate(found, axis=1)


def window_sums(footprints, latitudes, longitudes):
    """Summed area of the footprints whose centroid lies in each row's closed degree window."""
    lat, lon, area = footprints
    # A window across the antimeridian finds its far-side footprints as copies one turn away.
    far_west, far_east = lon < -180.0 + 2 * SEARCH_REACH_DEG, lon > 180.0 - 2 * SEARCH_REACH_DEG
    lat, area, true_lon = (np.concatenate([v, v[far_west], v[far_east]]) for v in (lat, area, lon))
    search_lon = np.concatenate([lon, lon[far_west] + 360.0, lon[far_east] - 360.0])
    # Latitude bands a quarter window tall, longitude-sorted inside: a window meets five bands, and
    # within a band its candidates are one contiguous run of the exact integer key band * count + rank.
    count, floor_lat, band_height = len(lat), latitudes.min() - SEARCH_REACH_DEG, SEARCH_REACH_DEG / 2
    by_lon = np.argsort(search_lon, kind="stable")
    lon_rank = np.empty(count, dtype=np.int64)
    lon_rank[by_lon] = np.arange(count)
    key = np.floor((lat - floor_lat) / band_height).astype(np.int64) * count + lon_rank
    by_key = np.argsort(key, kind="stable")
    key, lat, true_lon, area = key[by_key], lat[by_key], true_lon[by_key], area[by_key]
    centre = qmgrid.normalize_longitude(longitudes)
    rank_from = np.searchsorted(search_lon[by_lon], centre - SEARCH_REACH_DEG, side="left")
    rank_to = np.searchsorted(search_lon[by_lon], centre + SEARCH_REACH_DEG, side="right")
    first_band = np.floor((latitudes - SEARCH_REACH_DEG - floor_lat) / band_height).astype(np.int64)
    last_band = np.floor((latitudes + SEARCH_REACH_DEG - floor_lat) / band_height).astype(np.int64)
    sums = np.zeros(len(latitudes))
    for band in (first_band + step for step in range(int((last_band - first_band).max()) + 1)):
        run_from = np.searchsorted(key, band * count + rank_from, side="left")
        run_length = np.where(band <= last_band,
                              np.searchsorted(key, band * count + rank_to, side="left") - run_from, 0)
        run_end = np.cumsum(run_length)
        begin = 0
        while begin < len(sums):
            done = run_end[begin - 1] if begin else 0
            end = max(begin + 1, int(np.searchsorted(run_end, done + CANDIDATES_PER_STEP, side="right")))
            row = np.repeat(np.arange(end - begin, dtype=np.int32), run_length[begin:end])
            candidate = np.arange(done, run_end[end - 1]) + (run_from[begin:end] - run_end[begin:end]
                                                             + run_length[begin:end])[row]
            row_lat, candidate_lat = latitudes[begin:end][row], lat[candidate]
            in_strip = (candidate_lat >= row_lat - WINDOW_HALF_DEG) & (candidate_lat <= row_lat + WINDOW_HALF_DEG)
            row, candidate = row[in_strip], candidate[in_strip]
            # qmgrid.wrapped_longitude_delta bit for bit; numpy's float modulo is slow and only
            # antimeridian pairs need it.
            turned = (true_lon[candidate] - longitudes[begin:end][row]) + 180.0
            beyond = (turned < 0.0) | (turned >= 360.0)
            turned[beyond] %= 360.0
            inside = np.abs(turned - 180.0) <= WINDOW_HALF_DEG
            sums[begin:end] += np.bincount(row[inside], weights=area[candidate[inside]],
                                           minlength=end - begin)
            begin = end
    return sums


def window_areas_m2(prepared_dir, latitudes, longitudes):
    """Footprint area in each row's window; NaN where an intersected cell has no structures file."""
    if not len(latitudes):
        return np.zeros(0)
    corners = window_corner_squares(latitudes, longitudes)
    west = longitudes.min() - SEARCH_REACH_DEG
    bounds = (latitudes.min() - SEARCH_REACH_DEG, latitudes.max() + SEARCH_REACH_DEG,
              west, longitudes.max() + SEARCH_REACH_DEG - west)
    cells = {int(cell): cell_footprints(
        Path(prepared_dir) / qmgrid.square_name(*divmod(int(cell), qmgrid.Z9_AXIS)) / "structures.arrow", *bounds)
        for cell in np.unique(corners)}
    unbuilt = [cell for cell, footprints in cells.items() if footprints is None]
    sums = window_sums(np.concatenate([np.zeros((3, 0)), *(cells.pop(cell) for cell in list(cells)
                                                           if cell not in unbuilt)], axis=1), latitudes, longitudes)
    sums[np.isin(corners, unbuilt).any(axis=0)] = np.nan
    return sums


def built_up_classes(prepared_dir, latitudes, longitudes):
    """0 unknown, 1 rural, 2 urban: at least MIN_BUILT_PIXELS one-arcsecond pixels of footprint."""
    area = window_areas_m2(prepared_dir, latitudes, longitudes)
    # Scalar cosine: numpy's vector cosine may round the last bit differently.
    pixel_area = np.array([METRES_PER_DEG_LAT * METRES_PER_DEG_LON_EQ * math.cos(math.radians(lat)) / 3600**2
                           for lat in latitudes.tolist()])
    return np.where(np.isnan(area), 0, np.where(area / pixel_area >= MIN_BUILT_PIXELS, 2, 1)).astype(np.uint8)
