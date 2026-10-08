"""Venues of an OpenStreetMap extract (`venues.sh`), one line each: `lat lon kind outdoor_seating
area_m2 opening_hours name` (tab-separated, sorted by latitude then longitude), the point of a node
or the mean of an outline's vertices. Kinds: bar, pub, nightclub, biergarten, restaurant, cafe,
fast_food; outdoor_seating the tag's value, yes for an untagged venue with a mapped terrace
(leisure=outdoor_seating) within 30 m, which the leisure layer then leaves to it, else unknown;
area_m2 an outline's area, 0 for a node; opening_hours and name as mapped, empty when none. Reads
osmium's GeoJSON sequence on stdin.

    osmium export venues.osm.pbf -f geojsonseq | python3 venues.py venues.txt
"""
import json
import math
import re
import sys

KINDS = ('bar', 'pub', 'nightclub', 'biergarten', 'restaurant', 'cafe', 'fast_food')
# A mapped terrace this near a venue is its terrace (m; the builder's TERRACE_OF_VENUE_M).
TERRACE_OF_VENUE_M = 30.0
CONTROL = re.compile(r'[\x00-\x1f\x7f]+')


def centroid(geometry):
    """The mean of an outline's vertices (a point's own coordinates); None for anything else."""
    kind, coordinates = geometry.get('type'), geometry.get('coordinates')
    if kind == 'Point':
        return coordinates[1], coordinates[0]
    rings = {'Polygon': lambda c: c[:1], 'MultiPolygon': lambda c: [p[0] for p in c]}.get(kind)
    if rings is None:
        return None
    # A ring repeats its first vertex at its end: once is enough.
    points = [point for ring in rings(coordinates) for point in ring[:-1] or ring]
    if not points:
        return None
    return sum(p[1] for p in points) / len(points), sum(p[0] for p in points) / len(points)


def area_m2(geometry):
    """An outline's area (outer rings less holes, equirectangular at its first vertex); 0 else."""
    kind, coordinates = geometry.get('type'), geometry.get('coordinates')
    polygons = {'Polygon': lambda c: [c], 'MultiPolygon': lambda c: c}.get(kind)
    if polygons is None:
        return 0.0
    total = 0.0
    for polygon in polygons(coordinates):
        for index, ring in enumerate(polygon):
            if len(ring) < 3:
                continue
            scale = math.cos(math.radians(ring[0][1]))
            points = [(x * 111320.0 * scale, y * 110540.0) for x, y in ring]
            twice = sum(a[0] * b[1] - b[0] * a[1] for a, b in zip(points, points[1:] + points[:1]))
            total += abs(twice) / 2.0 * (1 if index == 0 else -1)
    return max(total, 0.0)


def seated_by_terraces(rows, terraces):
    """The rows with yes for an untagged venue that has a terrace within TERRACE_OF_VENUE_M."""
    cell = TERRACE_OF_VENUE_M / 111195.0
    grid = {}
    for lat, lon in terraces:
        grid.setdefault((int(lat // cell), int(lon // cell)), []).append((lat, lon))

    def near(lat, lon):
        scale = math.cos(math.radians(lat))
        span = int(1 / max(scale, 0.01)) + 1
        for i in (-1, 0, 1):
            for j in range(-span, span + 1):
                for t_lat, t_lon in grid.get((int(lat // cell) + i, int(lon // cell) + j), ()):
                    dy, dx = (t_lat - lat) * 111195.0, (t_lon - lon) * 111195.0 * scale
                    if math.hypot(dx, dy) <= TERRACE_OF_VENUE_M:
                        return True
        return False

    return [row[:3] + ('yes',) + row[4:] if row[3] == 'unknown' and near(row[0], row[1]) else row
            for row in rows]


def clean(text):
    """A tag's value on one line without tabs."""
    return CONTROL.sub(' ', text or '').strip()


def main(out_path):
    rows, terraces = [], []
    for line in sys.stdin:
        line = line.strip().lstrip('\x1e')
        if not line:
            continue
        feature = json.loads(line)
        tags = feature.get('properties') or {}
        kind = tags.get('amenity', '')
        geometry = feature.get('geometry') or {}
        point = centroid(geometry)
        if point is None:
            continue
        if kind not in KINDS:
            if tags.get('leisure') == 'outdoor_seating':
                terraces.append(point)
            continue
        seating = clean(tags.get('outdoor_seating')).lower() or 'unknown'
        rows.append((round(point[0], 6), round(point[1], 6), kind, seating,
                     round(area_m2(geometry)), clean(tags.get('opening_hours')),
                     clean(tags.get('name'))))
    rows = sorted(seated_by_terraces(rows, terraces))
    with open(out_path, 'w') as out:
        for lat, lon, kind, seating, area, hours, name in rows:
            out.write(f'{lat}\t{lon}\t{kind}\t{seating}\t{area}\t{hours}\t{name}\n')
    print(len(rows), 'venues,', len(terraces), 'terraces', file=sys.stderr)


if __name__ == '__main__':
    main(sys.argv[1])
