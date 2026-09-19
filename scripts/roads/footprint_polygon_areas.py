"""Ground areas of whole columns of structures_v4 grid polygons, decoded without a per-row loop."""

from pathlib import Path
import sys

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
import qmgrid  # noqa: E402
from prepared_arrow import grid_to_latlon  # noqa: E402

METRES_PER_DEG_LAT = 111_132.0
METRES_PER_DEG_LON_EQ = 111_320.0


MALFORMED = "Malformed structures_v4 building geometry"


def polygon_rings(counts, blob_starts, blob_ends):
    """(blob, first coordinate word, points, is exterior) of every ring, grouped by blob in ring order.

    All blobs advance together: one step reads each unfinished blob's next part header or ring,
    so the step count is the longest blob's ring count, not the row count.
    """
    cursor = blob_starts.copy()
    parts_left = np.ones(len(cursor), dtype=np.int64)
    rings_left = np.zeros(len(cursor), dtype=np.int64)
    exterior_next = np.zeros(len(cursor), dtype=bool)
    polygon_count_pending = np.ones(len(cursor), dtype=bool)
    active = np.arange(len(cursor))
    found = []
    while active.size:
        opens_part = rings_left[active] == 0
        finished = opens_part & (parts_left[active] == 0)
        if (cursor[active[finished]] != blob_ends[active[finished]]).any():
            raise ValueError(MALFORMED)
        active, opens_part = active[~finished], opens_part[~finished]
        if (cursor[active] >= blob_ends[active]).any():
            raise ValueError(MALFORMED)
        value = counts[cursor[active]].astype(np.int64)
        cursor[active] += 1
        remaining = blob_ends[active] - cursor[active]
        headers, header_value = active[opens_part], value[opens_part]
        if ((header_value < 1) | (header_value > remaining[opens_part])).any():
            raise ValueError(MALFORMED)
        is_polygon_count = polygon_count_pending[headers]
        parts_left[headers[is_polygon_count]] = header_value[is_polygon_count]
        polygon_count_pending[headers] = False
        parts, ring_count = headers[~is_polygon_count], header_value[~is_polygon_count]
        rings_left[parts], exterior_next[parts] = ring_count, True
        parts_left[parts] -= 1
        ringed, points = active[~opens_part], value[~opens_part]
        if ((points < 3) | (points > remaining[~opens_part] // 2)).any():
            raise ValueError(MALFORMED)
        found.append((ringed, cursor[ringed], points, exterior_next[ringed]))
        cursor[ringed] += 2 * points
        rings_left[ringed] -= 1
        exterior_next[ringed] = False
    if not found:
        return tuple(np.zeros(0, dtype=kind) for kind in (np.int64, np.int64, np.int64, bool))
    blob, first_word, points, exterior = (np.concatenate(column) for column in zip(*found))
    order = np.argsort(blob, kind="stable")
    return blob[order], first_word[order], points[order], exterior[order]


def footprint_areas_m2(geometries):
    """Exterior minus hole shoelace area of each non-null Arrow binary polygon, in a local metric frame."""
    if geometries.null_count:
        raise ValueError(MALFORMED)
    if not len(geometries):
        return np.zeros(0)
    offsets = np.frombuffer(geometries.buffers()[1], dtype=np.int32)[
        geometries.offset:geometries.offset + len(geometries) + 1].astype(np.int64)
    byte_lengths = np.diff(offsets)
    if ((byte_lengths % 4 != 0) | (byte_lengths == 0)).any():
        raise ValueError(MALFORMED)
    data = np.frombuffer(geometries.buffers()[2], dtype=np.uint8)[offsets[0]:offsets[-1]]
    word_offsets = (offsets - offsets[0]) // 4
    blob, first_word, points, exterior = polygon_rings(data.view("<u4"), word_offsets[:-1], word_offsets[1:])
    ring_start = np.cumsum(points) - points
    ring_of_point = np.repeat(np.arange(len(points)), points)
    word = first_word[ring_of_point] + 2 * (np.arange(len(ring_of_point)) - ring_start[ring_of_point])
    coordinates = data.view("<i4")
    lat, lon = grid_to_latlon(coordinates[word], coordinates[word + 1])
    metres_lon = METRES_PER_DEG_LON_EQ * np.cos(np.radians(lat[ring_start]))
    x = qmgrid.wrapped_longitude_delta(lon[ring_start][ring_of_point], lon) * metres_lon[ring_of_point]
    y = (lat - lat[ring_start][ring_of_point]) * METRES_PER_DEG_LAT
    following = np.arange(1, len(word) + 1)
    following[ring_start + points - 1] = ring_start
    ring_area = np.abs(np.add.reduceat(x * y[following], ring_start)
                       - np.add.reduceat(y * x[following], ring_start)) / 2
    first_ring = np.flatnonzero(np.diff(blob, prepend=-1))
    return np.maximum(np.add.reduceat(np.where(exterior, ring_area, -ring_area), first_ring), 0.0)
