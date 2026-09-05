#!/usr/bin/env python3
"""Shared integer-grid math for the pipeline scripts (mirrors engine/grid).

Web-Mercator z9 units, global int32 z30 cells, i64 differences. The Rust
crate is the spec; this file is the faithful copy — keep both in sync.
"""
import math

RADIUS_M = 6_378_137.0
CIRCUMFERENCE_M = 40_075_016.685_578_49
Z9_SPAN_DEG = 0.703_125
Z9_AXIS = 512
QUANTUM_M = CIRCUMFERENCE_M / 2**30
MAX_MERCATOR_LAT = 85.051_128_78
WIDE_RING_LAT = 81.9
MAX_HALO_KM = 11.0


def normalize_longitude(lon):
    return ((lon + 180.0) % 360.0) - 180.0


def wrapped_longitude_delta(from_lon, to_lon):
    return normalize_longitude(to_lon - from_lon)


def wrapped_longitude_midpoint(a_lon, b_lon):
    return normalize_longitude(a_lon + wrapped_longitude_delta(a_lon, b_lon) / 2.0)


def clamp_lat(lat):
    return max(-MAX_MERCATOR_LAT, min(MAX_MERCATOR_LAT, lat))


def lonlat_to_meters(lon, lat):
    lat = clamp_lat(lat)
    x = RADIUS_M * math.radians(lon)
    y = RADIUS_M * math.log(math.tan(math.pi / 4.0 + math.radians(lat) / 2.0))
    return x, y


def meters_to_lonlat(x, y):
    lon = math.degrees(x / RADIUS_M)
    lat = math.degrees(2.0 * math.atan(math.exp(y / RADIUS_M)) - math.pi / 2.0)
    return lon, lat


def meters_to_grid(x, y):
    gx = math.floor(x / QUANTUM_M) + (1 << 29)
    gy = math.floor(y / QUANTUM_M) + (1 << 29)
    assert -(1 << 31) <= gx < (1 << 31) and -(1 << 31) <= gy < (1 << 31)
    return gx, gy


def lonlat_to_grid(lon, lat):
    return meters_to_grid(*lonlat_to_meters(lon, lat))


def grid_to_meters(gx, gy):
    return (gx - (1 << 29)) * QUANTUM_M, (gy - (1 << 29)) * QUANTUM_M


def grid_to_lonlat(gx, gy):
    return meters_to_lonlat(*grid_to_meters(gx, gy))


def square_of(lat, lon):
    wrapped = normalize_longitude(lon)
    x = int((wrapped + 180.0) / 360.0 * Z9_AXIS)
    _, y_m = lonlat_to_meters(0.0, lat)
    half = CIRCUMFERENCE_M / 2.0
    y = int((half - y_m) / CIRCUMFERENCE_M * Z9_AXIS)
    return min(x, Z9_AXIS - 1), min(y, Z9_AXIS - 1)


def square_name(x, y):
    return f"z9/{x}/{y}"


def square_id(x, y):
    """Morton z9 identity, matching engine/grid::square_id."""
    if not (0 <= x < Z9_AXIS and 0 <= y < Z9_AXIS):
        raise ValueError("z9 coordinates must be in 0..511")
    return sum(((x >> bit) & 1) << (2 * bit) | ((y >> bit) & 1) << (2 * bit + 1)
               for bit in range(9))


def parse_square_name(name):
    rest = name.split("/", 1)
    if len(rest) != 2 or rest[0] != "z9":
        return None
    try:
        x, y = (int(v) for v in rest[1].split("/"))
    except ValueError:
        return None
    if not (0 <= x < Z9_AXIS and 0 <= y < Z9_AXIS):
        return None
    return x, y


def ring_radius(lat):
    return 1 if abs(lat) <= WIDE_RING_LAT else 2


def square_lonlat_span(x, y):
    """(lon0, lat_top, lon1, lat_bottom) degrees of a z9 unit."""
    lon0 = x * Z9_SPAN_DEG - 180.0
    lon1 = lon0 + Z9_SPAN_DEG
    half = CIRCUMFERENCE_M / 2.0
    top_m = half - y * (CIRCUMFERENCE_M / Z9_AXIS)
    bot_m = half - (y + 1) * (CIRCUMFERENCE_M / Z9_AXIS)
    _, lat_top = meters_to_lonlat(0.0, top_m)
    _, lat_bot = meters_to_lonlat(0.0, bot_m)
    return lon0, lat_top, lon1, lat_bot


def encode_grid_poly(ring):
    """[(gx,gy)] -> u32 LE count + int32 LE pairs (the `geom` column form)."""
    out = bytearray()
    out += len(ring).to_bytes(4, "little")
    for gx, gy in ring:
        out += int(gx).to_bytes(4, "little", signed=True)
        out += int(gy).to_bytes(4, "little", signed=True)
    return bytes(out)


def decode_grid_poly(blob):
    if blob is None or len(blob) < 4:
        return None
    n = int.from_bytes(blob[0:4], "little")
    # Two points = a wall segment; rings need three (area/contains guard that).
    if n < 2 or len(blob) != 4 + n * 8:
        return None
    ring = []
    for i in range(n):
        o = 4 + i * 8
        ring.append((
            int.from_bytes(blob[o:o + 4], "little", signed=True),
            int.from_bytes(blob[o + 4:o + 8], "little", signed=True),
        ))
    return ring


def ring_to_lonlat(ring):
    return [grid_to_lonlat(gx, gy) for gx, gy in ring]


def encode_grid_polygons(polygons):
    """Parts -> rings (exterior first) -> int32 pairs; mirrors engine/grid."""
    out = bytearray(len(polygons).to_bytes(4, "little"))
    for rings in polygons:
        out += len(rings).to_bytes(4, "little")
        for ring in rings:
            out += encode_grid_poly(ring)
    return bytes(out)


def decode_grid_polygons(blob):
    """Return all parts/rings or reject the entire malformed topology."""
    if blob is None:
        return None
    offset = 0

    def count():
        nonlocal offset
        if offset + 4 > len(blob):
            raise ValueError("truncated topology")
        value = int.from_bytes(blob[offset:offset + 4], "little")
        offset += 4
        return value

    try:
        polygon_count = count()
        if not 0 < polygon_count <= (len(blob) - offset) // 4:
            return None
        polygons = []
        for _ in range(polygon_count):
            ring_count = count()
            if not 0 < ring_count <= (len(blob) - offset) // 4:
                return None
            rings = []
            for _ in range(ring_count):
                start = offset
                points = count()
                if not 3 <= points <= (len(blob) - offset) // 8:
                    return None
                offset += points * 8
                rings.append(decode_grid_poly(blob[start:offset]))
            polygons.append(rings)
        return polygons if offset == len(blob) else None
    except ValueError:
        return None
