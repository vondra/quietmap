"""Road settlement density from the Overture screening stock in structures_v4."""

from collections import OrderedDict
import math
from pathlib import Path
import sys

import numpy as np
import pyarrow as pa

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
import qmgrid  # noqa: E402

# dev1's 17-pixel, 1-arcsecond raster window and 2026-08-30 vector calibration.
WINDOW_HALF_DEG = 8.5 / 3600
MIN_BUILT_PIXELS = 8
METRES_PER_DEG_LAT = 111_132.0
METRES_PER_DEG_LON_EQ = 111_320.0


def footprint_area_m2(blob):
    parts = qmgrid.decode_grid_polygons(blob)
    if parts is None:
        raise ValueError("Malformed structures_v4 building geometry")
    area = 0.0
    for rings in parts:
        for index, ring in enumerate(rings):
            points = qmgrid.ring_to_lonlat(ring)
            lon0, lat0 = points[0]
            metres_lon = METRES_PER_DEG_LON_EQ * math.cos(math.radians(lat0))
            x = np.array([qmgrid.wrapped_longitude_delta(lon0, lon) * metres_lon for lon, _ in points])
            y = np.array([(lat - lat0) * METRES_PER_DEG_LAT for _, lat in points])
            ring_area = abs(float(np.dot(x, np.roll(y, -1)) - np.dot(y, np.roll(x, -1)))) / 2
            area += ring_area if index == 0 else -ring_area
    return max(area, 0.0)


def window_squares(lat, lon):
    """All centroid-owner z9 cells intersected by the closed degree window."""
    west, north = qmgrid.square_of(lat + WINDOW_HALF_DEG, lon - WINDOW_HALF_DEG)
    east, south = qmgrid.square_of(lat - WINDOW_HALF_DEG, lon + WINDOW_HALF_DEG)
    return [(x % qmgrid.Z9_AXIS, y)
            for x in range(west, east + (qmgrid.Z9_AXIS if east < west else 0) + 1)
            for y in range(north, south + 1)]


class BuildingFootprintSampler:
    def __init__(self, prepared_dir):
        self.prepared_dir = Path(prepared_dir)
        # A whole owner can touch its 3x3 neighbourhood across the four corners.
        self.cells = OrderedDict()

    def cell_footprints(self, square):
        if square not in self.cells:
            self.cells[square] = self.load_cell(square)
            if len(self.cells) > 9:
                self.cells.popitem(last=False)
        self.cells.move_to_end(square)
        return self.cells[square]

    def load_cell(self, square):
        path = self.prepared_dir / qmgrid.square_name(*square) / "structures.arrow"
        try:
            source = pa.memory_map(str(path), "r")
        except FileNotFoundError:
            return None
        latitudes, longitudes, areas = [], [], []
        with source:
            reader = pa.ipc.open_file(source)
            metadata = reader.schema.metadata or {}
            if (metadata.get(b"structures_contract") != b"structures_v4"
                    or metadata.get(b"grid") != b"z30"):
                raise ValueError(f"{path}: expected grid z30 structures_v4")
            required = {"kind": pa.uint8(), "geom": pa.binary(), "osm_id": pa.int64(),
                        "centroid_gx": pa.int32(), "centroid_gy": pa.int32(),
                        "emission_centroid_gx": pa.int32(), "emission_centroid_gy": pa.int32()}
            for name, arrow_type in required.items():
                index = reader.schema.get_field_index(name)
                if index < 0 or reader.schema.field(index).type != arrow_type:
                    raise ValueError(f"{path}: invalid {name} column")
            for i in range(reader.num_record_batches):
                batch = reader.get_batch(i)
                for name in ("kind", "centroid_gx", "centroid_gy"):
                    if batch.column(name).null_count:
                        raise ValueError(f"{path}: null {name}")
                columns = {name: batch.column(name).to_pylist() for name in required}
                for row in range(batch.num_rows):
                    if columns["kind"][row] != 0:
                        continue
                    emission_x = columns["emission_centroid_gx"][row]
                    emission_y = columns["emission_centroid_gy"][row]
                    if (emission_x is None) != (emission_y is None):
                        raise ValueError(f"{path}: partial emission centroid")
                    # Matched rows retain Overture geometry; OSM-only rows do not.
                    if columns["osm_id"][row] is not None and emission_x is None:
                        continue
                    lon, lat = qmgrid.grid_to_lonlat(columns["centroid_gx"][row], columns["centroid_gy"][row])
                    latitudes.append(lat)
                    longitudes.append(qmgrid.normalize_longitude(lon))
                    areas.append(footprint_area_m2(columns["geom"][row]))
        # A latitude strip excludes distant rows without retaining any geometry.
        order = np.argsort(latitudes, kind="stable")
        return tuple(np.asarray(values, dtype=np.float64)[order]
                     for values in (latitudes, longitudes, areas))

    def window_area_m2(self, lat, lon):
        cells = [self.cell_footprints(square) for square in window_squares(lat, lon)]
        if any(cell is None for cell in cells):
            return None
        area = 0.0
        for latitudes, longitudes, areas in cells:
            start = np.searchsorted(latitudes, lat - WINDOW_HALF_DEG, side="left")
            end = np.searchsorted(latitudes, lat + WINDOW_HALF_DEG, side="right")
            inside = np.abs(qmgrid.wrapped_longitude_delta(lon, longitudes[start:end])) <= WINDOW_HALF_DEG
            area += float(areas[start:end][inside].sum())
        return area

    def classify(self, lat, lon):
        area = self.window_area_m2(lat, lon)
        if area is None:
            return 0
        pixel_area = METRES_PER_DEG_LAT * METRES_PER_DEG_LON_EQ * math.cos(math.radians(lat)) / 3600**2
        return 2 if area / pixel_area >= MIN_BUILT_PIXELS else 1
